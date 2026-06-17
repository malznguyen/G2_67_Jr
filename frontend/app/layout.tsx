import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "GMRAG2",
  description: "GMRAG2 frontend skeleton (T8).",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
