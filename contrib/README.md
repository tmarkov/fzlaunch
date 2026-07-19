# Contrib Plugins

This directory contains optional fzlaunch plugin executables.

## Recoll Content Search

`fzlaunch-recoll` searches a Recoll index through Recoll's Python API and emits
fzlaunch JSONL candidates. It is intended to be used as a triggered plugin
source:

```toml
[[sources.plugins]]
name = "recoll"
enabled = true
path = "/absolute/path/to/fzlaunch/contrib/fzlaunch-recoll"
selector = "r"
mode = "triggered"
direct_action = "xdg-open {}"
direct_action_execution = "detached"
```

Then entering a query such as:

```text
database checkpoint ;r
```

runs Recoll with `database checkpoint` as the query. Re-entering `;r` runs a
new search and fzlaunch stops the previous invocation.

Optional environment:

- `FZLAUNCH_RECOLL_LIMIT`: maximum Recoll results, default `50`.
- `RECOLL_CONFDIR`: standard Recoll config directory override.
- `FZLAUNCH_RECOLL_EXTRA_DBS`: optional `:`-separated list of additional Recoll
  Xapian database directories.

Dependencies:

- Python 3 available as `python` on `PATH`.
- Recoll's Python package, commonly shipped by distributions as
  `python3-recoll`.
- An existing Recoll index. If the plugin reports `Can't open index`, create or
  update the index with Recoll before running the plugin.
