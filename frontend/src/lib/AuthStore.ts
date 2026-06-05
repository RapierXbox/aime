import { create } from "zustand";

export type AimeAccount = {
  id: string;
  name: string;
  email: string;
};

type AuthStoreState = {
  account: AimeAccount | null;
};

type AuthStoreActions = {
  login: () => void;
};

type AuthStore = AuthStoreState & AuthStoreActions;

export const useAuthStore = create<AuthStore>()((set) => ({
  account: null as AimeAccount | null,
  login: () => {
    set({
      account: { id: "0", name: "Test User", email: "test@aime.ai" },
    });
  },
}));

export const isLoggedIn = (state: AuthStoreState): boolean =>
  state.account !== null;
