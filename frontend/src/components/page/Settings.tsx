import AccountSection from "./settings/AccountSection";
import ApplicationSection from "./settings/ApplicationSection";
import { Separator } from "../ui/separator";
import { LaptopMinimal, User, type LucideProps } from "lucide-react";
import { useNavigationStore } from "@/lib/NavigationStore";

/// the individual sections of the settings page
export type SettingsSection = "account" | "application";

const LoginPage: React.FC = () => {
  const openSettings = useNavigationStore((state) => state.openSettings);
  const currentSection = useNavigationStore((state) => state.settings);

  const CurrSectionComponent: React.FC | undefined = SECTIONS.find(
    (section) => section.id === currentSection,
  )?.component;

  if (!CurrSectionComponent)
    throw new Error(`Invalid settings section: ${currentSection}`);

  return (
    <div className="flex flex-1 min-h-0">
      <div
        id="sections"
        className="flex flex-col pt-4 p-2 pl-1.5 w-40 overflow-scroll bg-muted-secondary"
      >
        {SECTIONS.map((section) => (
          <button
            key={section.id}
            className="text-left text-sm font-heading hover:bg-muted rounded-sm p-1
            py-2 text-secondary-foreground flex justify-start items-center gap-1 "
            onClick={() => openSettings(section.id)}
          >
            <section.icon color="var(--muted-foreground)" size="16" />
            {section.name}
          </button>
        ))}
      </div>
      <Separator orientation="vertical" className="" />
      <div className="flex flex-1 p-2 overflow-y-scroll flex-col items-center">
        <div id="content" className="w-full max-w-[80ch]">
          <CurrSectionComponent />
        </div>
      </div>
    </div>
  );
};

const SECTIONS: {
  name: string;
  id: SettingsSection;
  component: React.FC;
  icon: React.FC<LucideProps>;
}[] = [
  {
    name: "Application",
    id: "application",
    component: ApplicationSection,
    icon: LaptopMinimal,
  },
  { name: "Account", id: "account", component: AccountSection, icon: User },
];

export default LoginPage;
