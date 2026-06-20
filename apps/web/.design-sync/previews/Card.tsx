import * as React from "react";
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
  CardFooter,
  Button,
} from "@hostlet/web";

export const Basic = () => (
  <Card style={{ maxWidth: 380 }}>
    <CardHeader>
      <CardTitle>Production</CardTitle>
      <CardDescription>Deployed 2 minutes ago · commit a1b2c3d</CardDescription>
    </CardHeader>
    <CardContent>
      <p style={{ fontSize: 14, color: "var(--muted)", margin: 0 }}>
        Your app is live and serving traffic. Push to <code>main</code> to ship a new build.
      </p>
    </CardContent>
    <CardFooter style={{ gap: 8 }}>
      <Button>View app</Button>
      <Button variant="secondary">Logs</Button>
    </CardFooter>
  </Card>
);

export const ContentOnly = () => (
  <Card style={{ maxWidth: 380 }}>
    <CardContent style={{ paddingTop: 20 }}>
      <div style={{ fontWeight: 600, color: "var(--ink)" }}>Storage</div>
      <div style={{ fontSize: 14, color: "var(--muted)", marginTop: 4 }}>2.4 GB of 10 GB used</div>
    </CardContent>
  </Card>
);
