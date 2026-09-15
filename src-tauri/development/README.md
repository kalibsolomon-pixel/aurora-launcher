Aurora Launcher — development release fixtures
=============================================

No production Aurora release infrastructure (manifest endpoint or artifact
repository) exists yet. This directory holds the launcher's checked-in
DEVELOPMENT release source: a release manifest fixture plus tiny synthetic
artifact files. Nothing here is a real Aurora build, and no production
release discovery is implied anywhere in the UI.

To use the development source end to end, serve this directory on the
loopback port the fixture manifest pins (8765), for example from this
directory:

    python -m http.server 8765 --bind 127.0.0.1

Then create an instance in the launcher's Instances panel (or via the
`create_instance` command). Artifact URLs are loopback cleartext, which the
launcher accepts only as its documented local development/test transport;
production release URLs will be HTTPS and will arrive together with the real
release infrastructure in a later phase.
