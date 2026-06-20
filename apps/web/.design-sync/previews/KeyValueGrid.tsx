import * as React from "react";
import { KeyValueGrid, KeyValueItem } from "@hostlet/web";
import { ExternalLink } from "lucide-react";

export const AppConfig = () => (
  <div style={{ maxWidth: 600 }}>
    <KeyValueGrid>
      <KeyValueItem label="Region" value="us-central1" />
      <KeyValueItem label="Machine" value="homelab-01" />
      <KeyValueItem label="Runtime" value="Docker 26.1" />
      <KeyValueItem label="Auto deploy" value="Enabled" />
    </KeyValueGrid>
  </div>
);

export const WithLinks = () => (
  <div style={{ maxWidth: 600 }}>
    <KeyValueGrid columns="md:grid-cols-3">
      <KeyValueItem
        label="Repository"
        value="acme/api-gateway"
        href="https://github.com/acme/api-gateway"
        externalIcon={<ExternalLink size={12} />}
      />
      <KeyValueItem label="Branch" value="main" />
      <KeyValueItem label="Last commit" value={<span style={{ fontFamily: "monospace", fontSize: 12 }}>4f9c2a1</span>} />
    </KeyValueGrid>
  </div>
);
