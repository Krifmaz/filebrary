# Jobs

> **Planned for Phase 3.** See [Feature status](../status.md).

Long-running work runs in a background job queue: conversions, indexing,
creating archives, and custom jobs. The **Jobs** page shows each job's
progress, and lets you cancel a running job or retry a failed one.

## Custom jobs

Administrators can define custom jobs in the configuration file: a program
and its arguments, run against selected files. They can only be defined in
the configuration file, never from the web UI, and Filebrary runs them
directly, never through a shell, so file names cannot inject commands.
