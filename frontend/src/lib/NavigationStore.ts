import { create } from "zustand";

export type Page =
  | {
      page: "login";
      // meaning this page should not be added to the navigation stack
      ephemeral: true;
    }
  | {
      page: "main";
    }
  | {
      page: "settings";
    };

type NavigationStoreState = {
  navStack: Page[];
  currentPageIndex: number;
};

type NavigationStoreActions = {
  navigateTo: (page: Page) => void;
  navigateBack: () => void;
  navigateForward: () => void;
};

type NavigationStore = NavigationStoreState & NavigationStoreActions;

/// stores the current navigation state
/// which popups are open,
/// which page is selected with forward/back navigation
// the navigation uses a stack. when navigating back, the index is increased
// to allow for forward navigation if needed
// when navigating to a new page, the stack is truncated so the new page is at the top of the stack
export const useNavigationStore = create<NavigationStore>()((set) => ({
  navStack: [{ page: "main" }],

  currentPageIndex: 0,
  navigateTo: (page: Page) => {
    set((state) => {
      let newNavStack: Page[];

      // if the current page is ephemeral, replace it with the new page
      if (state.navStack[state.currentPageIndex]["ephemeral"]) {
        newNavStack = [...state.navStack.slice(0, -1), page];
        // otherwise, check if the current page is at the top of the stack
      } else if (state.currentPageIndex < state.navStack.length - 1) {
        // if it is not, truncate the stack to the current page and push the new page
        newNavStack = [
          ...state.navStack.slice(0, state.currentPageIndex + 1),
          page,
        ];
      } else {
        // otherwise, push the new page onto the stack
        newNavStack = [...state.navStack, page];
      }

      return {
        currentPageIndex: newNavStack.length - 1,
        navStack: newNavStack,
      };
    });
  },

  navigateBack: () => {
    set((state) => {
      console.log(state);
      if (state.currentPageIndex === 0) {
        return {};
      }

      return {
        currentPageIndex: state.currentPageIndex - 1,
      };
    });
  },

  navigateForward: () => {
    set((state) => {
      if (state.currentPageIndex === state.navStack.length - 1) {
        return {};
      }

      return {
        currentPageIndex: state.currentPageIndex + 1,
      };
    });
  },
}));

export const selectCurrentPage = (state: NavigationStore): Page =>
  state.navStack[state.currentPageIndex];
