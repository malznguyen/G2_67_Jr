import { createNavigation } from "next-intl/navigation";

import { locales, defaultLocale, type Locale } from "./config";

export const { Link, redirect, usePathname, useRouter, getPathname } =
  createNavigation({
    locales: [...locales],
    defaultLocale,
    localePrefix: "as-needed",
  });

export type { Locale };
