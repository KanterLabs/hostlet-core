"use client";

import Link from "next/link";
import { Box, HardDrive, LogOut, Settings, ScrollText } from "lucide-react";
import { api } from "@/lib/api";

export function Nav() {
  const item = "flex items-center gap-2 rounded-md px-3 py-2 text-sm hover:bg-panel";
  async function logout() {
    await api("/api/logout", { method: "POST", body: "{}" }).catch(() => {});
    window.location.assign("/login");
  }
  return (
    <aside className="flex min-h-screen flex-col border-r border-line bg-white p-4">
      <Link href="/" className="mb-6 block text-lg font-semibold">Hostlet</Link>
      <nav className="space-y-1">
        <Link className={item} href="/servers"><HardDrive size={16}/>Machines</Link>
        <Link className={item} href="/apps"><Box size={16}/>Apps</Link>
        <Link className={item} href="/logs"><ScrollText size={16}/>Logs</Link>
        <Link className={item} href="/settings"><Settings size={16}/>Settings</Link>
      </nav>
      <button className="mt-auto flex items-center gap-2 bg-white px-3 py-2 text-sm text-neutral-800 ring-1 ring-line hover:bg-panel" onClick={logout}>
        <LogOut size={16}/>Log out
      </button>
    </aside>
  );
}
