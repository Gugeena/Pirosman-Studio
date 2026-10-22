// Native OS context menus (Tauri menu API) instead of the WebView's browser menu.
import { Menu, type CheckMenuItemOptions, type MenuItemOptions, type PredefinedMenuItemOptions, type SubmenuOptions } from "@tauri-apps/api/menu";

export type CtxItem =
  | "sep"
  | { text: string; action: () => void; enabled?: boolean; accelerator?: string; checked?: boolean }
  | { text: string; submenu: CtxItem[]; enabled?: boolean };

type Native = MenuItemOptions | CheckMenuItemOptions | PredefinedMenuItemOptions | SubmenuOptions;

function toNative(items: CtxItem[]): Native[] {
  return items.map((it): Native => {
    if (it === "sep") return { item: "Separator" };
    if ("submenu" in it) return { text: it.text, enabled: it.enabled ?? true, items: toNative(it.submenu) } as SubmenuOptions;
    const base = { text: it.text, enabled: it.enabled ?? true, accelerator: it.accelerator, action: () => it.action() };
    return it.checked === undefined ? base : { ...base, checked: it.checked };
  });
}

let open = false;

/** Show a native context menu at the pointer. Call from an onContextMenu handler. */
export async function showContextMenu(e: { preventDefault(): void; stopPropagation(): void }, items: CtxItem[]) {
  e.preventDefault();
  e.stopPropagation();
  if (open) return;
  open = true;
  try {
    const menu = await Menu.new({ items: toNative(items) });
    await menu.popup();
  } finally {
    open = false;
  }
}

/** Standard edit menu for text fields. */
export const textFieldItems = (): Native[] => [
  { item: "Undo" },
  { item: "Redo" },
  { item: "Separator" },
  { item: "Cut" },
  { item: "Copy" },
  { item: "Paste" },
  { item: "Separator" },
  { item: "SelectAll" },
];

export async function showTextFieldMenu(e: MouseEvent) {
  e.preventDefault();
  if (open) return;
  open = true;
  try {
    const menu = await Menu.new({ items: textFieldItems() });
    await menu.popup();
  } finally {
    open = false;
  }
}
