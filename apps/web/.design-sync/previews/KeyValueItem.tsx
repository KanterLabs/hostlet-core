import * as React from "react";
import { KeyValueItem } from "@hostlet/web";
import { ExternalLink } from "lucide-react";

export const Standalone = () => (
  <div style={{ maxWidth: 220, border: "1px solid var(--line, #e5e7eb)" }}>
    <KeyValueItem label="Region" value="us-central1" />
  </div>
);

export const WithLink = () => (
  <div style={{ maxWidth: 240, border: "1px solid var(--line, #e5e7eb)" }}>
    <KeyValueItem
      label="Repository"
      value="acme/storefront"
      href="https://github.com/acme/storefront"
      externalIcon={<ExternalLink size={12} />}
    />
  </div>
);

export const NotSet = () => (
  <div style={{ maxWidth: 220, border: "1px solid var(--line, #e5e7eb)" }}>
    <KeyValueItem label="Custom domain" />
  </div>
);
