import { redirect } from "next/navigation";

import { defaultLocale } from "@/i18n/config";

// Root entry: bounce to the default locale so [locale] layout takes over.
export default function RootPage() {
  redirect(`/${defaultLocale}`);
}
