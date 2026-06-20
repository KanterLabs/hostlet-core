import * as React from "react";
import { Card, CardHeader, CardTitle, CardDescription } from "@hostlet/web";

export const InContext = () => (
  <Card style={{ maxWidth: 400 }}>
    <CardHeader>
      <CardTitle>Storage usage</CardTitle>
      <CardDescription>Across all apps in your workspace</CardDescription>
    </CardHeader>
  </Card>
);
