export const locales = ["vi", "en"] as const;
export type Locale = (typeof locales)[number];

export const defaultLocale: Locale = "vi";

export function isLocale(value: string | undefined | null): value is Locale {
  return value !== null && value !== undefined && (locales as readonly string[]).includes(value);
}
