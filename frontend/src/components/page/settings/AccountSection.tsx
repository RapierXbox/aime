import { Separator } from "@/components/ui/separator";

import React from "react";
import googleSigninLight from "@/assets/google_signin_light.svg";
import googleSigninDark from "@/assets/google_signin_dark.svg";
import { Button } from "@/components/ui/button";
import { commands, type ListEmailEntry } from "@/bindings";
import { Progress as ProgressBar } from "radix-ui";
import { useQuery } from "@tanstack/react-query";
import { qk } from "@/lib/queryKeys";
import { accountSyncState, useSyncStore } from "@/lib/SyncStore";

const syncBtnText = {
  idle: "Sync",
  syncing: "⏳",
  ok: "☑️",
  error: "❌",
} as const;

// a singular entry with sync and refresh buttons
const EmailAccEntry: React.FC<{ acc: ListEmailEntry }> = ({ acc }) => {
  const { status, progress, statusMessage } = useSyncStore((s) =>
    accountSyncState(s, acc.id),
  );
  const sync = useSyncStore((s) => s.sync);

  return (
    <>
      <div className="flex flex-col gap-3 py-3">
        <div className="flex items-center gap-3">
          <div
            className="h-9 w-9 shrink-0 bg-muted text-muted-foreground
                 flex items-center justify-center text-sm font-medium"
          >
            {acc.name.substring(0, 2).toLocaleUpperCase()}
          </div>

          <span className="font-medium mr-auto">{acc.name}</span>

          <Button onClick={() => sync(acc.id)}>{syncBtnText[status]}</Button>
          <Button variant="destructive" onClick={() => sync(acc.id, true)}>
            Full Sync
          </Button>
        </div>

        {progress !== null && (
          <div className="flex flex-col gap-1.5">
            {statusMessage !== null && (
              <span className="text-xs text-shimmer">{statusMessage}</span>
            )}
            <ProgressBar.Root
              value={progress * 100}
              className="w-full h-1.5 bg-muted overflow-hidden rounded-full"
            >
              <ProgressBar.Indicator
                className="h-full bg-primary transition-transform rounded-full"
                style={{ transform: `translateX(-${100 - progress * 100}%)` }}
              />
            </ProgressBar.Root>
          </div>
        )}
      </div>

      <Separator
        orientation="horizontal"
        className="-mx-2 data-horizontal:w-[calc(100%+1rem)]"
      />
    </>
  );
};

const AccountSettings: React.FC = () => {
  const email_accs = useQuery({
    queryKey: qk.accounts,
    queryFn: () =>
      commands.emailListAccounts().then((it) => {
        if (it.status === "error") throw it.error;
        return it.data;
      }),
  });

  const user = {
    name: "John Doe",
    email: "john.doe@example.com",
  };

  return (
    <section className="flex flex-col gap-2 w-full">
      <span className="text-2xl font-heading">Account</span>
      <div className="flex ">
        <div
          id="user-avatar"
          className="h-16 w-16 rounded-full bg-muted
                 text-muted-foreground flex items-center justify-center
                 outline outline-border text-2xl"
        >
          {user.name.substring(0, 2).toLocaleUpperCase()}
        </div>
        <div className="flex flex-col ml-4 gap-1">
          <span className="font-medium">{user.name}</span>
          <span className="text-sm text-muted-foreground">{user.email}</span>
        </div>
      </div>
      <span className="text-xl font-heading">E-Mail Accounts</span>
      <div className="flex flex-col">
        <div id="" className="flex flex-row py-3">
          <span className="">Add Account</span>
          <button
            className="ml-auto"
            onClick={() => {
              commands.registerGmailAccount().then((it) => {
                console.log(it);
                if (it.status === "error") {
                  throw it.error;
                }

              });
            }}
          >
            <img
              src={googleSigninLight}
              alt="Sign in with Google"
              className="block dark:hidden"
            />
            <img
              src={googleSigninDark}
              alt="Sign in with Google"
              className="hidden dark:block"
            />
          </button>
        </div>

        <Separator
          orientation="horizontal"
          className="-mx-2 data-horizontal:w-[calc(100%+1rem)]"
        />
        {email_accs.data?.map((acc) => (
          <EmailAccEntry acc={acc} key={acc.id} />
        ))}
      </div>
    </section>
  );
};

export default AccountSettings;
