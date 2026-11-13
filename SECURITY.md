# Security Policy

aivyx-toolbench holds `aivyx-stencil`, which turns an agent's tool
list into a grammar the model server enforces, and the benchmark that
measures it. A way to make a constrained model emit a call to a tool that
isn't in its list, or to break out of the grammar into arbitrary text the
agent then acts on, is in scope.

This is not a bug bounty program.

## Reporting a vulnerability

Report it privately on GitHub: [Report a vulnerability](https://github.com/Aivyx-Agent/aivyx-toolbench/security/advisories/new)
(this repo's **Security** tab). Only the maintainer sees the report
until a fix is published. Please don't open a public issue for a
vulnerability.

We aim to resolve or provide a remediation plan for a confirmed
vulnerability within 90 days of the report, or coordinate a later
disclosure date with the reporter if a fix genuinely needs longer.
Credit is offered in release notes at the reporter's preference.
