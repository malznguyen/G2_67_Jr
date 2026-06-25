// Root pass-through layout. The <html>/<body> + providers live in
// src/app/[locale]/layout.tsx (i18n locale segment). Next App Router requires
// a root layout file; this one renders children only and lets the [locale]
// segment own the document shell.
export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return children;
}
