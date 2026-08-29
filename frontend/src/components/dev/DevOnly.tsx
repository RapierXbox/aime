interface DevOnlyProps {
  children: React.ReactNode;
}

export const DevOnly: React.FC<DevOnlyProps> = ({ children }) => {
  const isDev = import.meta.env.DEV;

  if (!isDev) return null;

  return <>{children}</>;
};
