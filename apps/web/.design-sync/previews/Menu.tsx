import * as React from "react";
import { Menu, MenuButton } from "@hostlet/web";

const noop = () => {};

// Menu owns its open state internally, so the static card shows the closed
// trigger (the real, honest render). Items are wired so the API reads true.
export const Default = () => (
  <div style={{ display: "flex", gap: 16, alignItems: "center" }}>
    <Menu>
      <MenuButton onSelect={noop}>View logs</MenuButton>
      <MenuButton onSelect={noop}>Restart app</MenuButton>
      <MenuButton onSelect={noop}>Delete app</MenuButton>
    </Menu>
    <Menu trigger={<span>Actions</span>}>
      <MenuButton onSelect={noop}>Redeploy</MenuButton>
      <MenuButton onSelect={noop}>Roll back</MenuButton>
    </Menu>
  </div>
);
