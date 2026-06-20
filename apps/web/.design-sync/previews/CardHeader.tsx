import * as React from "react";
import { Card, CardHeader, CardTitle, CardDescription } from "@hostlet/web";

export const WithDescription = () => (
  <Card style={{ maxWidth: 400 }}>
    <CardHeader>
      <CardTitle>notes-app</CardTitle>
      <CardDescription>Deployed 3 minutes ago · commit d4e5f6a · us-east-1</CardDescription>
    </CardHeader>
  </Card>
);

export const TitleOnly = () => (
  <Card style={{ maxWidth: 400 }}>
    <CardHeader>
      <CardTitle>Environment variables</CardTitle>
    </CardHeader>
  </Card>
);
