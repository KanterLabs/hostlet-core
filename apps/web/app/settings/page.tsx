import { Nav } from "@/components/Nav";

export default function Settings() {
  return (
    <main className="grid min-h-screen grid-cols-[220px_1fr]">
      <Nav />
      <section className="p-8">
        <h1 className="text-2xl font-semibold">Settings</h1>
        <div className="mt-4 rounded-lg border border-line bg-white p-4">
          <h2 className="font-medium">Environment, domain, and health path</h2>
          <p className="muted mt-2">Use the app detail API to update encrypted environment variables, domain, and health path for an app.</p>
        </div>
      </section>
    </main>
  );
}
