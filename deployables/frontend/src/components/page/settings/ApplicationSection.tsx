import React from "react";

const ApplicationSection: React.FC = () => {
  return (
    <section className="flex flex-col gap-2 w-full">
      <span className="text-2xl font-heading">Application</span>
      <div className="w-full h-32 rounded-md bg-muted"></div>
    </section>
  );
};

export default ApplicationSection;
