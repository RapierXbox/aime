import { useEffect, useState } from "react";
import { Button } from "../ui/button";
import { Bug } from "lucide-react";
import { cn } from "@/lib/utils";

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
      className="group data-[collapsed=false]:border-2 border-red-200 p-2 rounded-md"
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
