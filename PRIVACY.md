# Privacy and data

Marquee connects to Plex services and the Plex Media Servers selected in the app. The app keeps account and server credentials, preferences and diagnostic information locally. Settings includes a complete, localized privacy notice and a control to delete local data.

Crash reporting and usage analytics are separate choices. Both default to off. When a build is configured for reporting and the relevant choice is enabled, crash reports use Sentry and usage analytics use PostHog's EU endpoint. A sign-in problem report can also be sent as a one-off when requested from the sign-in screen. Builds without the corresponding reporting configuration cannot send to that service.

Turning reporting off removes its local identifier and queued reports; a report already being sent can still complete. Removing local data does not remove reports already received by another service. Signing out removes the account's stored credentials and reporting choices; uninstalling alone may leave data outside the application directory.

Source and issue tracker: https://github.com/nphil/Marquee
