// Server-side dictionary loader used by layouts/tests.
import type { Locale } from "./config";

import vi from "./messages/vi.json";
import en from "./messages/en.json";

const dictionaries: Record<Locale, Record<string, unknown>> = {
  vi: vi as Record<string, unknown>,
  en: en as Record<string, unknown>,
};

export type Dictionary = typeof vi;

export function getDictionary(locale: string): Record<string, unknown> {
  return dictionaries[locale as Locale] ?? dictionaries.vi;
}
