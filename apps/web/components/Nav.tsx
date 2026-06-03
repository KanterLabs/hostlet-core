"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { Box, HardDrive, Home, LogOut, Settings, ScrollText, TerminalSquare, LucideIcon } from "lucide-react";
import { api } from "@/lib/api";
import { cx } from "@/components/ui";

type NavItem = { href: string; label: string; icon: LucideIcon };

const NAV_ITEMS: NavItem[] = [
  { href: "/", label: "Overview", icon: Home },
  { href: "/apps", label: "Apps", icon: Box },
  { href: "/servers", label: "Machines", icon: HardDrive },
  { href: "/logs", label: "Logs", icon: ScrollText },
  { href: "/settings", label: "Settings", icon: Settings },
];

function isActive(pathname: string, href: string) {
  return href === "/" ? pathname === "/" : pathname.startsWith(href);
}

function NavLink({
  item,
  className,
  iconSize,
}: {
  item: NavItem;
  className: string;
  iconSize: number;
}) {
  const Icon = item.icon;
  return (
    <Link href={item.href} className={className}>
      <Icon size={iconSize} />
      {item.label}
    </Link>
  );
}

const LOGOUT_CLASS =
  "mt-3 w-full justify-start border-white/10 bg-white/5 text-neutral-100 shadow-none hover:bg-white/10";

export function Nav() {
  const pathname = usePathname();

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
            <span className="text-xs font-medium text-neutral-400">self-hosted deploys</span>
          </span>
        </Link>
        <nav className="space-y-1">
          {NAV_ITEMS.map((item) => {
            const active = isActive(pathname, item.href);
            return (
              <NavLink
                key={item.href}
                item={item}
                iconSize={17}
                className={cx(
                  "flex min-h-10 items-center gap-2 rounded-md px-3 py-2 text-sm font-medium transition",
                  active ? "bg-white/10 text-white ring-1 ring-white/10" : "text-neutral-300 hover:bg-white/5 hover:text-white",
                )}
              />
            );
          })}
        </nav>
        <div className="mt-auto rounded-lg border border-white/10 bg-white/5 p-3">
          <div className="text-sm font-medium">Control plane</div>
          <p className="mt-1 text-sm text-neutral-400">Local-first management for your own servers.</p>
        </div>
        <button className={LOGOUT_CLASS} onClick={logout}>
          <LogOut size={16} />
          Log out
        </button>
      </aside>

      <nav className="fixed inset-x-0 bottom-0 z-30 grid grid-cols-5 border-t border-white/10 bg-rail px-2 pb-[calc(0.5rem+env(safe-area-inset-bottom))] pt-2 text-white shadow-lg shadow-neutral-950/20 lg:hidden">
        {NAV_ITEMS.map((item) => {
          const active = isActive(pathname, item.href);
          return (
            <NavLink
              key={item.href}
              item={item}
              iconSize={17}
              className={cx(
                "flex flex-col items-center gap-1 rounded-md px-2 py-1.5 text-[11px] font-medium",
                active ? "bg-white/10 text-white" : "text-neutral-400",
              )}
            />
          );
        })}
      </nav>
    </>
  );
}
