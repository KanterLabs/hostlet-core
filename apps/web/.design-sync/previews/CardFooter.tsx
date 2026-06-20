import * as React from "react";
import { Card, CardHeader, CardTitle, CardDescription, CardContent, CardFooter, Button } from "@hostlet/web";

export const InContext = () => (
  <Card style={{ maxWidth: 400 }}>
    <CardHeader>
      <CardTitle>notes-app</CardTitle>
      <CardDescription>Running · us-east-1 · commit a1b2c3d</CardDescription>
    </CardHeader>
    <CardContent>
      <p style={{ fontSize: 14, color: "var(--muted)", margin: 0 }}>
        Your app is live and serving traffic on notes.hostlet.cloud.
      </p>
    </CardContent>
    <CardFooter style={{ gap: 8 }}>
      <Button>View app</Button>
      <Button variant="secondary">Logs</Button>
    </CardFooter>
  </Card>
);
