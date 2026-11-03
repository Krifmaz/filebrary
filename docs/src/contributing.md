# Contributing

Pull requests are welcome. Before opening one:

- Run the checks listed in [Building from source](install/source.md#running-the-checks).
- Add tests for new behaviour (unit tests, plus integration tests against a
  temporary directory).
- Document every public item with doc comments, and every new setting in
  these docs.
- Never pass user input through a shell.

CI runs formatting, clippy, tests and the web build on Linux, macOS and
Windows, builds the Docker image and checks its size, and builds these docs.

Contributions are accepted under the Apache License, version 2.0.
