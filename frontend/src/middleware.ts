import createIntlMiddleware from "next-intl/middleware";
import { NextResponse, type NextRequest } from "next/server";

import { auth } from "@/auth";
import { locales, defaultLocale, isLocale } from "@/i18n/config";

const intlMiddleware = createIntlMiddleware({
  locales: [...locales],
  defaultLocale,
  localePrefix: "as-needed",
  localeDetection: true,
});

// Routes that require an authenticated session. Tenant/workspace app shell.
const PROTECTED_PATTERN =
  /^\/(?:[a-z]{2}\/)?t\/(?:[^/]+)(?:\/w\/(?:[^/]+))?(?:\/.*)?$/;

function localeOf(pathname: string): string {
  const seg = pathname.split("/")[1];
  return isLocale(seg) ? seg : defaultLocale;
}

export default async function middleware(req: NextRequest) {
  // 1) i18n locale routing first (may rewrite/redirect to localized path).
  const intlResponse = intlMiddleware(req);
  const pathname = req.nextUrl.pathname;

  // 2) Auth guard on protected tenant routes.
  if (PROTECTED_PATTERN.test(pathname)) {
    const session = await auth();
    if (!session) {
      const locale = localeOf(pathname);
      const url = req.nextUrl.clone();
      url.pathname = `/${locale}/login`;
      url.search = "";
      const res = NextResponse.redirect(url);
      // Carry over cookies set by intl middleware (locale cookie).
      intlResponse?.cookies.getAll().forEach((c) => res.cookies.set(c));
      return res;
    }
  }

  return intlResponse ?? NextResponse.next();
}

export const config = {
  matcher: [
    // Run on everything except Next internals, API, auth, and static assets.
    "/((?!api|_next|_vercel|.*\\..*|auth).*)",
  ],
};
