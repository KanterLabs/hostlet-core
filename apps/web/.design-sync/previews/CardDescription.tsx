import * as React from "react";
import { Card, CardHeader, CardTitle, CardDescription } from "@hostlet/web";

export const InContext = () => (
  <Card style={{ maxWidth: 400 }}>
    <CardHeader>
      <CardTitle>api-gateway</CardTitle>
      <CardDescription>Last deployed 2 hours ago · 3 replicas · Frankfurt, DE</CardDescription>
    </CardHeader>
  </Card>
);
