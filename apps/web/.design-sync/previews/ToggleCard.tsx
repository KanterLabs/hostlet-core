import * as React from "react";
import { ToggleCard } from "@hostlet/web";
import { Globe, Bell } from "lucide-react";

const col: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 12, maxWidth: 420 };

export const Unchecked = () => {
  const [checked, setChecked] = React.useState(false);
  return (
    <div style={col}>
      <ToggleCard
        checked={checked}
        onChange={setChecked}
        icon={Globe}
        label="Expose to public internet"
        description="Make this app reachable at your custom domain or a *.hostlet.cloud subdomain."
      />
    </div>
  );
};

export const Checked = () => {
  const [checked, setChecked] = React.useState(true);
  return (
    <div style={col}>
      <ToggleCard
        checked={checked}
        onChange={setChecked}
        icon={Bell}
        label="Deploy alerts"
        description="Send Slack and email notifications when a deployment starts or fails."
      />
    </div>
  );
};
