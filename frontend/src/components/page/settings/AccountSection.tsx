import { Separator } from "@/components/ui/separator";

import React, { useEffect } from "react";
import googleSigninLight from "@/assets/google_signin_light.svg";
import googleSigninDark from "@/assets/google_signin_dark.svg";
import { Button } from "@/components/ui/button";
import { DevOnly } from "@/components/dev/DevOnly";
import { commands, type ListEmailEntry } from "@/bindings";

/*
/// The user's email address.
#[serde(rename = "emailAddress")]
pub email_address: Option<String>,
/// The ID of the mailbox's current history record.
#[serde(rename = "historyId")]
#[serde_as(as = "Option<serde_with::DisplayFromStr>")]
pub history_id: Option<u64>,
/// The total number of messages in the mailbox.
#[serde(rename = "messagesTotal")]
pub messages_total: Option<i32>,
/// The total number of threads in the mailbox.
#[serde(rename = "threadsTotal")]
pub threads_total: Option<i32>,*/

const AccountSettings: React.FC = () => {
  // const _user = useAuthStore((state) => state.account);
  const user = {
    name: "John Doe",
    email: "john.doe@example.com",
  };

  const [accs, setAccs] = React.useState<ListEmailEntry[]>([]);

  const load_email_accs = () =>
    commands.emailListAccounts().then((it) => {
      if (it.status === "ok") {
        setAccs(it.data);
      } else {
        throw it.error;
      }
    });

  useEffect(() => {
    load_email_accs();
  }, []);

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
      <div className="flex flex-col gap-2">
        <div id="" className="flex flex-row">
          <span className="">Add Account</span>
          <button
            className="ml-auto"
            onClick={() => {
              commands.registerGmailAccount().then((it) => {
                console.log(it);
                if (it.status === "error") {
                  throw it.error;
                }

                load_email_accs();
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

        <Separator orientation="horizontal" />
        {accs.map((acc) => (
          <div key={acc.id}>
            <span className="font-medium">
              {acc.name}
              <DevOnly>
                <Button
                  onClick={() => commands.devEmailFullSync(acc.id)}
                  variant="destructive"
                >
                  Onboard Sync
                </Button>
              </DevOnly>
            </span>
          </div>
        ))}
      </div>
    </section>
  );
};

export default AccountSettings;
