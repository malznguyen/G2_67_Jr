// Root pass-through layout. The actual <html>/<<body> + providers live in
// src/app/[locale]/layout.tsx (i18n locale segment). This file exists so Next
// App Router has a stable root entry; it renders children only.
export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return children;
}
