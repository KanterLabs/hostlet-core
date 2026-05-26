"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useEffect, useState } from "react";
import { Box, HardDrive, Home, LogOut, Settings, ScrollText, TerminalSquare } from "lucide-react";
import { api } from "@/lib/api";

export function Nav() {
  const pathname = usePathname();
  const [mode, setMode] = useState<"self_hosted" | "cloud">("self_hosted");
  const items = [
    { href: "/", label: "Overview", icon: Home },
    { href: "/apps", label: "Apps", icon: Box },
    ...(mode === "cloud" ? [] : [{ href: "/servers", label: "Machines", icon: HardDrive }]),
    { href: "/logs", label: "Logs", icon: ScrollText },
    { href: "/settings", label: "Settings", icon: Settings },
  ];

  useEffect(() => {
    api<{ mode: "self_hosted" | "cloud" }>("/api/session")
      .then((session) => setMode(session.mode))
      .catch(() => {});
  }, []);

  async function logout() {
    await api("/api/logout", { method: "POST", body: "{}" }).catch(() => {});
    window.location.assign("/login");
  }

  return (
    <>
      <aside className="hidden min-h-screen flex-col border-r border-white/10 bg-rail p-4 text-white shadow-xl shadow-neutral-950/10 lg:flex">
        <Link href="/" className="mb-6 flex items-center gap-3 rounded-lg px-2 py-2 transition hover:bg-white/5">
          <span className="flex h-9 w-9 items-center justify-center rounded-lg bg-action text-white ring-1 ring-white/15">
            <TerminalSquare size={19} />
          </span>
          <span>
            <span className="block text-lg font-semibold leading-5">Hostlet</span>
            <span className="text-xs font-medium text-neutral-400">{mode === "cloud" ? "cloud deploys" : "self-hosted deploys"}</span>
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
                  active ? "bg-white/10 text-white ring-1 ring-white/10" : "text-neutral-300 hover:bg-white/5 hover:text-white"
                }`}
              >
                <Icon size={17} />
                {item.label}
              </Link>
            );
          })}
        </nav>
        <div className="mt-auto rounded-lg border border-white/10 bg-white/5 p-3">
          <div className="text-sm font-medium">{mode === "cloud" ? "Hostlet Cloud" : "Control plane"}</div>
          <p className="mt-1 text-sm text-neutral-400">{mode === "cloud" ? "Managed always-on deploys on Hostlet compute." : "Local-first management for your own servers."}</p>
        </div>
        <button className="mt-3 w-full justify-start border-white/10 bg-white/5 text-neutral-100 shadow-none hover:bg-white/10" onClick={logout}>
          <LogOut size={16} />
          Log out
        </button>
      </aside>

      <nav className={`fixed inset-x-0 bottom-0 z-30 grid ${mode === "cloud" ? "grid-cols-4" : "grid-cols-5"} border-t border-white/10 bg-rail px-2 pb-[calc(0.5rem+env(safe-area-inset-bottom))] pt-2 text-white shadow-lg shadow-neutral-950/20 lg:hidden`}>
        {items.map((item) => {
          const Icon = item.icon;
          const active = item.href === "/" ? pathname === "/" : pathname.startsWith(item.href);
          return (
            <Link
              key={item.href}
              href={item.href}
              className={`flex flex-col items-center gap-1 rounded-md px-2 py-1.5 text-[11px] font-medium ${
                active ? "bg-white/10 text-white" : "text-neutral-400"
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
