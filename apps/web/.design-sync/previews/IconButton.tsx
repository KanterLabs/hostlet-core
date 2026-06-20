import * as React from "react";
import { Trash2, RefreshCw, Settings, ExternalLink } from "lucide-react";
import { IconButton } from "@hostlet/web";

export const Examples = () => (
  <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
    <IconButton label="Redeploy app">
      <RefreshCw size={16} />
    </IconButton>
    <IconButton label="App settings">
      <Settings size={16} />
    </IconButton>
    <IconButton label="Open in browser">
      <ExternalLink size={16} />
    </IconButton>
    <IconButton label="Delete app">
      <Trash2 size={16} />
    </IconButton>
  </div>
);
