# Security policy

## Supported versions

Security fixes go into the latest release. Upgrade before reporting, in case the problem is
already fixed.

| Version | Supported |
| --- | --- |
| 0.2.0 | Yes |
| Older | No |

## Reporting a vulnerability

Report it privately, not in a public issue: use the
[Report a vulnerability](https://github.com/elfenomenokim/jlogtail/security/advisories/new) form on
the repository's Security tab.

Include the command, the version (`jlogtail --version`), your platform, and what an attacker could
do with the result. You get a reply within a few days. Say how you want to be credited if you want a
name in the release note.

## In scope

- Reading bytes outside the requested file and offset, including through a symlink, a rotation or a
  relative path that resolves somewhere unexpected.
- Emitting a record that another process appended later, or data from a file the caller did not
  name.
- Memory that grows with the file rather than with the requested window, so a large transcript can
  exhaust the process.
- `--follow` resuming at an offset that skips or repeats a record after a rotation.

## Not a vulnerability

- Content of the transcript reaching your own terminal or your own pipeline. The tool prints the
  records you asked for.
- A filter that returns nothing, or a byte window that starts mid-record and skips that row.
- Reading a file your user account can already read.

## Disclosure

Once a fix is released, the advisory is published with credit to the reporter unless they prefer
otherwise.
