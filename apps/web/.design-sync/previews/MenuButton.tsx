import * as React from "react";
import { MenuButton } from "@hostlet/web";

const noop = () => {};

// MenuButton is a single menu item; shown in a panel that mimics an open menu.
export const Items = () => (
  <div
    style={{
      width: 240,
      display: "grid",
      gap: 8,
      border: "1px solid var(--line)",
      borderRadius: 8,
      padding: 8,
      background: "var(--surface)",
      boxShadow: "0 10px 24px rgba(12,14,13,0.12)",
    }}
  >
    <MenuButton onSelect={noop}>View logs</MenuButton>
    <MenuButton onSelect={noop}>Restart app</MenuButton>
    <MenuButton onSelect={noop}>Delete app</MenuButton>
  </div>
);
