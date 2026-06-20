import * as React from "react";
import { FilterTabs } from "@hostlet/web";
import { Server } from "lucide-react";

const STATUSES = ["all", "running", "stopped", "failed"] as const;
type Status = typeof STATUSES[number];

export const Default = () => {
  const [status, setStatus] = React.useState<Status>("running");
  return (
    <FilterTabs<Status>
      label="Status"
      value={status}
      items={STATUSES}
      onChange={setStatus}
    />
  );
};

export const WithIcon = () => {
  const [status, setStatus] = React.useState<Status>("all");
  return (
    <FilterTabs<Status>
      label="Apps"
      value={status}
      items={STATUSES}
      onChange={setStatus}
      icon={Server}
    />
  );
};
