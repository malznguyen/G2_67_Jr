import { getRequestConfig } from "next-intl/server";

import { isLocale, defaultLocale } from "./config";

export default getRequestConfig(async ({ requestLocale }) => {
  const requested = await requestLocale;
  const locale = isLocale(requested) ? requested : defaultLocale;
  const messages = (await import(`./messages/${locale}.json`)).default as Record<
    string,
    unknown
  >;

  return { locale, messages };
});
