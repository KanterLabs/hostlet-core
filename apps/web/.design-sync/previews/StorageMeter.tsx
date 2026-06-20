import * as React from "react";
import { StorageMeter } from "@hostlet/web";

export const LowUsage = () => (
  <div style={{ maxWidth: 400 }}>
    <StorageMeter
      label="Managed storage"
      usedBytes={2573741824}
      limitBytes={10737418240}
    />
  </div>
);

export const HighUsage = () => (
  <div style={{ maxWidth: 400 }}>
    <StorageMeter
      label="Managed storage"
      usedBytes={8053063680}
      limitBytes={10737418240}
    />
  </div>
);

export const OverLimit = () => (
  <div style={{ maxWidth: 400 }}>
    <StorageMeter
      label="Managed storage"
      usedBytes={11811160064}
      limitBytes={10737418240}
    />
  </div>
);
