"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { Box, HardDrive, Home, LogOut, Settings, ScrollText, TerminalSquare } from "lucide-react";
import { api } from "@/lib/api";

export function Nav() {
  const pathname = usePathname();
  const items = [
    { href: "/", label: "Overview", icon: Home },
    { href: "/apps", label: "Apps", icon: Box },
    { href: "/servers", label: "Machines", icon: HardDrive },
    { href: "/logs", label: "Logs", icon: ScrollText },
    { href: "/settings", label: "Settings", icon: Settings },
  ];

  async function logout() {
    await api("/api/logout", { method: "POST", body: "{}" }).catch(() => {});
    window.location.assign("/login");
  }

  return (
    <>
      <aside className="hidden min-h-screen flex-col border-r border-line bg-white/92 p-4 shadow-sm shadow-neutral-950/5 backdrop-blur lg:flex">
        <Link href="/" className="mb-6 flex items-center gap-3 rounded-lg px-2 py-2">
          <span className="flex h-9 w-9 items-center justify-center rounded-lg bg-ink text-white">
            <TerminalSquare size={19} />
          </span>
          <span>
            <span className="block text-lg font-semibold leading-5">Hostlet</span>
            <span className="eyebrow">self-hosted deploys</span>
          </span>
        </Link>
        <nav className="space-y-1">
          {items.map((item) => {
            const Icon = item.icon;
            const active = item.href === "/" ? pathname === "/" : pathname.startsWith(item.href);
            return (
              <Link
                key={item.href}
                href={item.href}
                className={`flex min-h-10 items-center gap-2 rounded-md px-3 py-2 text-sm font-medium transition ${
                  active ? "bg-emerald-50 text-action ring-1 ring-emerald-100" : "text-neutral-700 hover:bg-panel hover:text-ink"
                }`}
              >
                <Icon size={17} />
                {item.label}
              </Link>
            );
          })}
        </nav>
        <div className="mt-auto rounded-lg border border-line bg-panel p-3">
          <div className="text-sm font-medium">Control plane</div>
          <p className="muted mt-1">Local-first management for your own servers.</p>
        </div>
        <button className="button-secondary mt-3 w-full justify-start" onClick={logout}>
          <LogOut size={16} />
          Log out
        </button>
      </aside>

      <nav className="fixed inset-x-0 bottom-0 z-30 grid grid-cols-5 border-t border-line bg-white/95 px-2 py-2 shadow-lg shadow-neutral-950/10 backdrop-blur lg:hidden">
        {items.map((item) => {
          const Icon = item.icon;
          const active = item.href === "/" ? pathname === "/" : pathname.startsWith(item.href);
          return (
            <Link
              key={item.href}
              href={item.href}
              className={`flex flex-col items-center gap-1 rounded-md px-2 py-1.5 text-[11px] font-medium ${
                active ? "bg-emerald-50 text-action" : "text-neutral-600"
              }`}
            >
              <Icon size={17} />
              {item.label}
            </Link>
          );
        })}
      </nav>
    </>
  );
}
