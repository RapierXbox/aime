import { create } from "zustand";
import { commands, type Draft, type DraftFields } from "@/bindings";
import { emailError, unwrap } from "./drafts";

// Client-side edit buffers for drafts. The Rust side owns the canonical draft
// (SQLite + Gmail); this store holds what the user is typing and pushes it back
// in two debounced stages:
//   keystroke --500ms--> flushDraft (saveDraft: local DB, revision-checked)
//             --2s/account--> syncDraftAccount (syncDrafts: upload to Gmail)
// Fresh server copies come in through `adopt`, which must never clobber
// unsaved edits.

export type DraftBuffer = {
  /** Last copy received from the backend (revision, status, account, ...). */
  draft: Draft;
  /** What the editor shows; may be ahead of `draft.fields`. */
  fields: DraftFields;
  /** `fields` has edits not yet saved. */
  dirty: boolean;
  /** Bumped on every local edit, so a finished save can tell whether the user
   * kept typing while it was in flight. */
  generation: number;
  saving: boolean;
  error: string | null;
  /** The saved draft changed underneath unsaved edits; saving is blocked
   * until the user resolves it (see DraftComposer). */
  remoteChanged: boolean;
  /** Bumped when `fields` is replaced from outside the editor; used as the
   * editor's React key so it remounts with the new text. */
  editorVersion: number;
};

type DraftState = {
  syncErrors: Record<string, string | null>;
  syncing: Record<string, boolean>;
  buffers: Record<string, DraftBuffer>;
  adopt: (d: Draft) => void;
  change: (id: string, fields: Partial<DraftFields>) => void;
  discardBuffer: (id: string, draft: Draft) => void;
};

// Module-level, not in the store: none of this needs to trigger re-renders.
// Debounced local save per draft id.
const saveTimers = new Map<string, ReturnType<typeof setTimeout>>();
// Debounced Gmail upload per account id.
const uploadTimers = new Map<string, ReturnType<typeof setTimeout>>();
// Fields currently being saved per draft id, so `adopt` can recognise the
// echo of our own save and not flag it as a remote change.
const savingSnapshots = new Map<string, DraftFields>();
// In-flight saveDraft per draft id; serialises saves of the same draft.
const writes = new Map<string, Promise<Draft>>();

export const useDraftStore = create<DraftState>()((set, get) => ({
  buffers: {},
  syncErrors: {},
  syncing: {},
  
  // Take in a draft fetched from the backend.
  adopt: (draft) => {
    set((s) => {
      const old = s.buffers[draft.id];
      // Stale response that lost a race with a newer one: ignore.
      if (old && draft.revision < old.draft.revision) return {};
      // User has unsaved/in-flight edits: keep their `fields`, only refresh
      // metadata. If the new content is neither what we had nor what we're
      // currently saving, someone else changed it -> flag a conflict.
      if (old?.dirty || old?.saving) {
        const changed =
          JSON.stringify(draft.fields) !== JSON.stringify(old.draft.fields) &&
          JSON.stringify(draft.fields) !==
            JSON.stringify(savingSnapshots.get(draft.id));
        return {
          buffers: {
            ...s.buffers,
            [draft.id]: {
              ...old,
              draft,
              remoteChanged: old.remoteChanged || changed,
            },
          },
        };
      }
      // Clean buffer: replace with the backend copy.
      const fields = draft.fields;
      return {
        buffers: {
          ...s.buffers,
          [draft.id]: {
            draft,
            fields,
            dirty: false,
            generation: (old?.generation ?? 0) + 1,
            saving: false,
            error: null,
            remoteChanged: false,
            // Only remount the editor if the text actually changed.
            editorVersion:
              (old?.editorVersion ?? 0) +
              (JSON.stringify(old?.fields) === JSON.stringify(fields) ? 0 : 1),
          },
        },
      };
    });
  },
  // Editor input: apply locally and (re)start the 500ms save debounce.
  change: (id, fields) => {
    const old = get().buffers[id];
    if (!old) return;
    patch(id, {
      fields: { ...old.fields, ...fields },
      dirty: true,
      generation: old.generation + 1,
      error: null,
    });
    saveLater(id);
    // Prevent an older coalesced upload from running while the user continues typing.
    clearTimeout(uploadTimers.get(old.draft.account_id));
  },
  // Throw away local edits and load `draft` instead (conflict resolution,
  // after send, ...).
  discardBuffer: (id, draft) => {
    clearTimeout(saveTimers.get(id));
    set((s) => {
      const buffers = { ...s.buffers };
      delete buffers[id];
      return { buffers };
    });
    get().adopt(draft);
  },
}));

// Shallow-merge into one buffer.
function patch(id: string, values: Partial<DraftBuffer>) {
  useDraftStore.setState((s) => ({
    buffers: { ...s.buffers, [id]: { ...s.buffers[id], ...values } },
  }));
}
// Debounce local saves per draft: a burst of keystrokes -> one saveDraft.
function saveLater(id: string) {
  clearTimeout(saveTimers.get(id));
  saveTimers.set(
    id,
    setTimeout(() => {
      saveTimers.delete(id);
      void flushDraft(id).catch(() => {});
    }, 500),
  );
}
// Debounce Gmail uploads per account: many local saves -> one sync.
function uploadLater(account: string) {
  clearTimeout(uploadTimers.get(account));
  uploadTimers.set(
    account,
    setTimeout(() => {
      uploadTimers.delete(account);
      void syncDraftAccount(account).catch(() => {});
    }, 2000),
  );
}
// In-flight sync per account; concurrent callers share it.
const syncs = new Map<string, Promise<void>>();

// Upload this account's local drafts to Gmail (and pull remote changes), then
// re-read every open buffer of the account so status/revision are current.
export async function syncDraftAccount(account: string): Promise<void> {
  const running = syncs.get(account);

  if (running) return running;

  useDraftStore.setState((s) => ({
    syncing: { ...s.syncing, [account]: true },
  }));

  const work = (async () => {
    try {
      await unwrap(commands.syncDrafts(account));
      useDraftStore.setState((s) => ({
        syncErrors: { ...s.syncErrors, [account]: null },
      }));
    } catch (e) {
      useDraftStore.setState((s) => ({
        syncErrors: { ...s.syncErrors, [account]: emailError(e) },
      }));
      throw e;
    } finally {
      const ids = Object.values(useDraftStore.getState().buffers)
        .filter((b) => b.draft.account_id === account)
        .map((b) => b.draft.id);
      for (const id of ids) {
        try {
          useDraftStore.getState().adopt(await unwrap(commands.getDraft(id)));
        } catch {
          /* A failed detail read does not erase the editor. */
        }
      }
      useDraftStore.setState((s) => ({
        syncing: { ...s.syncing, [account]: false },
      }));
    }
  })();
  syncs.set(account, work);
  try {
    await work;
  } finally {
    syncs.delete(account);
  }
}

// Save a buffer's edits to the local DB now (skipping the debounce).
// Resolves with the saved draft; throws on conflict or backend error.
export async function flushDraft(id: string): Promise<Draft> {
  clearTimeout(saveTimers.get(id));
  saveTimers.delete(id);
  // Only one save per draft at a time: wait for the current one, then retry
  // with whatever the buffer holds by then.
  const running = writes.get(id);
  if (running) {
    await running;
    return flushDraft(id);
  }
  const buffer = useDraftStore.getState().buffers[id];
  if (!buffer) throw new Error("Draft buffer missing");
  if (!buffer.dirty) return buffer.draft;
  // Refuse to overwrite a draft that changed remotely or isn't in an editable
  // state; the composer shows the error and offers keep-as-copy / reload.
  if (
    buffer.remoteChanged ||
    [
      "Conflict",
      "Missing",
      "Unknown",
      "Sending",
      "Unsupported",
      "Sent",
    ].includes(buffer.draft.status)
  ) {
    patch(id, {
      error:
        "The saved version changed. Keep your edits as a separate draft or load the saved version.",
    });
    throw new Error("Draft conflict");
  }
  // `buffer` is a snapshot: edits made while saving bump `generation` on the
  // live buffer and are picked up by the re-flush below.
  savingSnapshots.set(id, buffer.fields);
  patch(id, { saving: true });
  const work = (async () => {
    try {
      // Backend rejects the save if `revision` is no longer current.
      const saved = await unwrap(
        commands.saveDraft(id, buffer.draft.revision, buffer.fields),
      );
      const current = useDraftStore.getState().buffers[id];
      patch(id, {
        draft: saved,
        // Still dirty if the user typed during the save.
        dirty: current.generation !== buffer.generation,
        saving: false,
        error: null,
      });
      uploadLater(saved.account_id);
      return saved;
    } catch (err) {
      patch(id, { saving: false, error: emailError(err) });
      // Refresh revision/status so the UI can show why it failed.
      try {
        useDraftStore.getState().adopt(await unwrap(commands.getDraft(id)));
      } catch {
        /* Keep the original buffer on failure. */
      }
      throw err;
    }
  })();
  writes.set(id, work);
  try {
    await work;
  } finally {
    writes.delete(id);
    savingSnapshots.delete(id);
  }
  // Edits arrived during the save: save again.
  if (
    useDraftStore.getState().buffers[id]?.dirty &&
    !useDraftStore.getState().buffers[id]?.remoteChanged
  )
    return flushDraft(id);
  return useDraftStore.getState().buffers[id].draft;
}
// Save every dirty buffer, one after another (before navigating away or closing the window).
export async function flushAllDrafts() {
  for (const id of Object.keys(useDraftStore.getState().buffers)) {
    if (useDraftStore.getState().buffers[id].dirty) await flushDraft(id);
  }
}
