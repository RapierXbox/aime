import { clsx, type ClassValue } from "clsx"
import {
  Inbox,
  PencilLine,
  Send,
  Shredder,
  Star,
  Trash,
  Trash2,
} from "lucide-react";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

// The available
export const EMAIL_INBOXES = [
  { id: "INBOX", name: "Inbox", icon: Inbox } as const,
  { id: "SENT", name: "Sent", icon: Send } as const,
  { id: "DRAFT", name: "Draft", icon: PencilLine } as const,
  { id: "STARRED", name: "Starred", icon: Star } as const,
  { id: "TRASH", name: "Trash", icon: Trash2 } as const,
  { id: "SPAM", name: "Spam", icon: Shredder } as const,
] as const;
