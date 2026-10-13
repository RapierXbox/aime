import { useQuery } from "@tanstack/react-query";
import { commands } from "@/bindings";
import { qk } from "@/lib/queryKeys";
import { emailError, unwrap } from "@/lib/drafts";
import { useNavigationStore } from "@/lib/NavigationStore";

export default function DraftsPage({ accountId }: { accountId: string }) {
  const query = useQuery({
    queryKey: qk.drafts(accountId),
    queryFn: () => unwrap(commands.listDrafts(accountId)),
  });
  const panel = useNavigationStore((s) => s.rightPanel);
  const select = useNavigationStore((s) => s.selectDraft);
  return (
    <div className="w-[20rem] shrink-0 flex flex-col overflow-y-auto border-r border-border">
      <h2 className="p-3 text-sm font-semibold border-b border-border">
        Drafts
      </h2>
      {query.isPending && <p className="p-3 text-sm">Loading drafts…</p>}
      {query.isError && (
        <p role="alert" className="p-3 text-sm text-destructive">
          {emailError(query.error)}
        </p>
      )}
      {query.data?.length === 0 && (
        <p className="p-3 text-sm text-muted-foreground">No saved drafts</p>
      )}
      {query.data?.map((d) => (
        <button
          type="button"
          key={d.id}
          className={`text-left p-3 border-b border-border hover:bg-muted ${panel?.view === "draft" && panel.draftId === d.id ? "bg-muted" : ""}`}
          onClick={() => void select(accountId, d.id).catch(() => {})}
        >
          <p className="text-sm font-semibold truncate">
            {d.fields.subject || "(No subject)"}
          </p>
          <p className="text-xs truncate">
            {d.fields.to || d.fields.cc || d.fields.bcc || "(No recipients)"}
          </p>
          <p className="text-xs text-muted-foreground">
            {d.status === "Unknown" ? "Gmail status unknown" : d.status}
          </p>
        </button>
      ))}
    </div>
  );
}
