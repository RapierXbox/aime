import { useEffect, useState } from "react";
import { Bug } from "lucide-react";

interface DevOnlyProps {
  children: React.ReactNode;
}

export const DevOnly: React.FC<DevOnlyProps> = ({ children }) => {
  useEffect(() => {

  });
  const isDev = import.meta.env.DEV;
  const [isCollapsed, setIsCollapsed] = useState(true);
  if (!isDev) return null;

  return (
    <div
      className="group data-[collapsed=false]:border-2 border-red-200 data-collapsed:p-2 
        rounded-md overflow-x-scroll"
      data-collapsed={isCollapsed}
    >
      <Bug
        className="transition-transform duration-300 
                   group-data-[collapsed=false]:rotate-180 select-none"
        size={16}
        onClick={() => setIsCollapsed(!isCollapsed)}
      />
      {!isCollapsed && children}
    </div>
  );
};
