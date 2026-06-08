import { useAuthStore } from "@/lib/AuthStore";
import React from "react";

const AccountSettings: React.FC = () => {
  const user = useAuthStore((state) => state.account);

  return (
    <section className="flex flex-col gap-2 w-full">
      <span className="text-2xl font-heading">Account</span>
      <div className="w-full h-32 rounded-md bg-muted"></div>
    </section>
  );
};

export default AccountSettings;
