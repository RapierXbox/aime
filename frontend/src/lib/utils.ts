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

export const shallowCompare = (obj1, obj2) => {
  if (Object.is(obj1, obj2)) return true;
  if (typeof obj1 !== 'object' || obj1 === null || typeof obj2 !== 'object' || obj2 === null) return false;

  const keys1 = Object.keys(obj1);
  const keys2 = Object.keys(obj2);

  if (keys1.length !== keys2.length) return false;

  return keys1.every(key => Object.prototype.hasOwnProperty.call(obj2, key) && Object.is(obj1[key], obj2[key]));
};

// The available
export const EMAIL_INBOXES = [
  { id: "INBOX", name: "Inbox", icon: Inbox } as const,
  { id: "SENT", name: "Sent", icon: Send } as const,
  { id: "DRAFT", name: "Draft", icon: PencilLine } as const,
  { id: "STARRED", name: "Starred", icon: Star } as const,
  { id: "TRASH", name: "Trash", icon: Trash2 } as const,
  { id: "SPAM", name: "Spam", icon: Shredder } as const,
] as const;
