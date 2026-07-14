# Deployment Reliability

Hostlet treats deployment as a recoverable state machine rather than one long
request. Protocol-v2 jobs use a rotating claim token and a renewable lease. An
expired worker cannot prepare or commit activation, and agents heartbeat while
building so long valid deployments are not mistaken for abandoned work.

## Activation

The agent builds and health-checks a deterministic candidate without changing
the live app. It then asks the API to prepare activation. The API locks the app,
checks the expected current deployment and claim token, records a pending
deployment, and allocates a monotonically increasing route generation. Only
that generation may be routed and committed. The final database transaction
updates the current deployment, service inventory, job status, and audit event
together.

Route writes are serialized and versioned. Runtime health repair is suppressed
while activation is pending, preventing an older health observation from
overwriting a newer route.

## Crash Recovery And Cancellation

Before execution, the agent fsyncs a secret-free journal in its work directory.
Phase transitions and activation generations are persisted there. If the agent
loses the API acknowledgement after switching a route, restart recovery retries
the idempotent commit instead of declaring the live release failed. A journal is
removed only after the API acknowledges terminal completion.

Cancellation is cooperative for running jobs and immediate for queued jobs.
The heartbeat response carries the cancellation request; dropping a running
Docker or Compose command kills its child process. Lease-renewal failure also
stops work before another agent can safely reclaim it.

## Compose Releases

Compose backing services use a stable `hostlet-app-*` project, network, and
named volumes. Web candidates use `hostlet-release-*` projects attached to that
stable network. Cleanup removes stale release projects but preserves the stable
layer; deleting the app intentionally removes both. A keyed backing-spec
fingerprint gates changes to databases, caches, volumes, and related service
configuration behind explicit maintenance approval.

## Upgrade Behavior

Agents advertise their supported protocol when claiming work. Protocol-v2 jobs
remain queued for a v2 agent, while older agents can continue claiming legacy
jobs. This makes API-first upgrades safe without allowing an old worker to skip
the activation fence.
