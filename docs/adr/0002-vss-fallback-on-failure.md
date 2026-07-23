# Fall back to Direct Read when a Shadow Copy can't be created

Creating a VSS Shadow Copy requires administrator privileges. When it fails for any reason (not elevated, VSS service disabled, an unsupported Volume such as a network share) `auto_backup` logs a warning and reads that Volume directly instead of failing the whole backup.

We chose fail-soft over fail-hard because breaking backups that worked before VSS was added felt worse than silently missing the new lock-safety benefit — many existing Task Scheduler setups are not configured to run elevated, and requiring that would be a breaking change for them. The tradeoff is that a task which is never run elevated will never actually benefit from VSS, with only the log line as a signal; the `auto_backup.log` entry ("VSS unavailable ...; falling back to direct read") is the intended way to notice this, and the README calls out enabling "Run with highest privileges" in Task Scheduler.
