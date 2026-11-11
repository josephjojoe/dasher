export const metadata = {
  title: "Hydration in a headless browser",
  description: "A Next.js App Router page",
};

export default function RootLayout({ children }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
