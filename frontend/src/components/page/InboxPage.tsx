const InboxPage: React.FC<{
  accountId: string;
  inboxId: string;
}> = ({ accountId, inboxId }) => {
  return (
    <div>
      Inbox {inboxId} of {accountId}
    </div>
  );
};

export default InboxPage;
