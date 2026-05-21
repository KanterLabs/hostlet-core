import Link from "next/link";
import { Box, HardDrive, Settings, ScrollText } from "lucide-react";

export function Nav() {
  const item = "flex items-center gap-2 rounded-md px-3 py-2 text-sm hover:bg-panel";
  return (
    <aside className="min-h-screen border-r border-line bg-white p-4">
      <Link href="/" className="mb-6 block text-lg font-semibold">Hostlet</Link>
      <nav className="space-y-1">
        <Link className={item} href="/servers"><HardDrive size={16}/>Machines</Link>
        <Link className={item} href="/apps"><Box size={16}/>Apps</Link>
        <Link className={item} href="/logs"><ScrollText size={16}/>Logs</Link>
        <Link className={item} href="/settings"><Settings size={16}/>Settings</Link>
      </nav>
    </aside>
  );
}
