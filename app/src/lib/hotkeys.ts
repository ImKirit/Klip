// Key combinations in the spelling the recorder parses ("Alt+Shift+F10") and the checks
// the settings page runs on them. Must stay in step with recorder/src/hotkeys.rs.

import { t, tOpt } from "./i18n";
import type { HotkeyAction, HotkeyBinding } from "./types";

const MODIFIERS = ["Ctrl", "Alt", "Shift", "Win"] as const;

/** Names the recorder knows besides letters, digits and F1 to F24. */
const NAMED_KEYS: Record<string, string> = {
  Space: "Space",
  Tab: "Tab",
  Insert: "Insert",
  Delete: "Delete",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  Pause: "Pause",
  PrintScreen: "Print",
  ScrollLock: "Scroll",
};

const ALIASES: Record<string, string> = {
  STRG: "CTRL",
  CONTROL: "CTRL",
  UMSCHALT: "SHIFT",
  LEERTASTE: "SPACE",
  EINFG: "INSERT",
  ENTF: "DELETE",
  POS1: "HOME",
  ENDE: "END",
  DRUCK: "PRINT",
  ROLLEN: "SCROLL",
};

/** Builds the combination from a key press. Null while only modifiers are held. */
export function comboFromEvent(event: KeyboardEvent): { keys: string | null; unsupported: string | null } {
  if (["Control", "Alt", "Shift", "Meta", "AltGraph"].includes(event.key)) return { keys: null, unsupported: null };
  let key: string | null = null;
  // `code` names the physical key, so Shift+1 stays "1" and not "!".
  if (/^Key[A-Z]$/.test(event.code)) key = event.code.slice(3);
  else if (/^Digit[0-9]$/.test(event.code)) key = event.code.slice(5);
  else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(event.code)) key = event.code;
  else if (NAMED_KEYS[event.code]) key = NAMED_KEYS[event.code];
  if (!key) return { keys: null, unsupported: event.key.length === 1 ? event.key : event.code };
  const parts: string[] = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  if (event.metaKey) parts.push("Win");
  parts.push(key);
  return { keys: parts.join("+"), unsupported: null };
}

interface Parsed {
  mods: string[];
  key: string;
}

export function parseCombo(keys: string): Parsed | { error: string } {
  const mods: string[] = [];
  let key: string | null = null;
  for (const raw of keys.split("+").map((p) => p.trim())) {
    const upper = ALIASES[raw.toUpperCase()] ?? raw.toUpperCase();
    const mod = MODIFIERS.find((m) => m.toUpperCase() === upper);
    if (mod) {
      if (!mods.includes(mod)) mods.push(mod);
      continue;
    }
    if (key !== null) return { error: t("hk.errMany") };
    const known =
      /^F([1-9]|1[0-9]|2[0-4])$/.test(upper) ||
      /^[A-Z0-9]$/.test(upper) ||
      Object.values(NAMED_KEYS).some((n) => n.toUpperCase() === upper);
    if (!known) return { error: raw === "" ? t("hk.errNone") : t("hk.errUnknown", raw) };
    key = upper;
  }
  if (key === null) return { error: t("hk.errNone") };
  return { mods: MODIFIERS.filter((m) => mods.includes(m)), key };
}

/** Same combination, however it is spelled. */
export function comboId(keys: string): string | null {
  const parsed = parseCombo(keys);
  return "error" in parsed ? null : [...parsed.mods, parsed.key].join("+").toUpperCase();
}

export interface HotkeyProblem {
  level: "error" | "warning";
  text: string;
}

export function actionLabel(action: HotkeyAction): string {
  switch (action.type) {
    case "clip":
      return t("hk.aClip", action.seconds >= 60 && action.seconds % 60 === 0 ? `${action.seconds / 60} min` : `${action.seconds} s`);
    case "record_toggle":
      return t("hk.aRecordToggle");
    case "record_pause":
      return t("hk.aRecordPause");
    case "marker":
      return t("hk.aMarker");
    case "screenshot":
      return t("hk.aScreenshot");
    case "overlay":
      return t("hk.aOverlay");
    case "input_pause":
      return t("hk.aInputPause");
  }
}

/** Every action besides the clip keys. Each exists once at most. */
const FIXED_ACTIONS = ["record_toggle", "record_pause", "marker", "screenshot", "overlay", "input_pause"] as const;

/** Adds an empty binding for every action the list lacks, so an older config.json
 *  still offers the actions that came later. */
export function withAllActions(bindings: HotkeyBinding[]): HotkeyBinding[] {
  const missing = FIXED_ACTIONS.filter((type) => !bindings.some((b) => b.action.type === type));
  return missing.length === 0 ? bindings : [...bindings, ...missing.map((type) => ({ keys: "", action: { type } }))];
}

/** One list of problems per binding, in the order of the bindings. */
export function checkHotkeys(bindings: HotkeyBinding[]): HotkeyProblem[][] {
  const ids = bindings.map((b) => comboId(b.keys));
  return bindings.map((binding, index) => {
    const problems: HotkeyProblem[] = [];
    if (binding.keys.trim() === "") {
      problems.push({ level: "warning", text: t("hk.pNone") });
      return problems;
    }
    const parsed = parseCombo(binding.keys);
    if ("error" in parsed) {
      problems.push({ level: "error", text: t("hk.pInvalid", parsed.error) });
      return problems;
    }
    const twin = ids.findIndex((id, i) => i !== index && id !== null && id === ids[index]);
    if (twin >= 0) {
      problems.push({ level: "error", text: t("hk.pConflict", actionLabel(bindings[twin].action)) });
    }
    if (parsed.mods.length === 0 && !/^F\d+$/.test(parsed.key) && !["PRINT", "PAUSE", "SCROLL"].includes(parsed.key)) {
      problems.push({ level: "warning", text: t("hk.pBare") });
    }
    return problems;
  });
}

/** A combination as key caps, with the key names of the language ("Ctrl" or "Strg").
 *  Named keys have an entry "key.<NAME>" in the translation table. */
export function keyCaps(keys: string): string[] {
  return keys
    .split("+")
    .map((p) => p.trim())
    .filter(Boolean)
    .map((p) => ALIASES[p.toUpperCase()] ?? p.toUpperCase())
    .map((id) => tOpt(`key.${id}`) ?? id);
}
