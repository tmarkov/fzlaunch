# Overview

`fzlaunch` is a search-first, object-verb Linux launcher focused on minimizing
the keystrokes necessary to execute an action, and the amount of shell syntax,
paths, command names, and flags the user needs to have memorized.

The core interaction is:

1. Search for a value.
2. Optionally edit that value.
3. Optionally queue it.
4. Compose it with another value.
5. Execute the resulting shell command.

The launcher is intentionally shell-oriented. It constructs and runs commands
with shell-equivalent observable behavior.

# UI

When started, `fzlaunch` opens a TUI with:

1. Search box: text input with the cursor in it. It is always at the top.
2. Status line: the current queue/command composition, shown as final shell text.
   It is hidden when there is nothing to show.
3. Result box: sorted matching values, with one value selected. The best match is
   selected by default.
4. Preview box: a preview of the selected value. It is visible by default and can
   be hidden. When hidden, the result box expands to full width.

# Values

Everything that can be selected, queued, inserted, or executed behaves as a
value.

When a value is edited with `[backtick]`, the search box shows editable text.
When a value is inserted into a command, it is inserted either verbatim or with
POSIX shell quoting.

Examples:

| Value | Search result | Editable text | Inserted shell text |
| --- | --- | --- | --- |
| Firefox command | executables | `firefox` | `firefox` |
| File | files | `/home/me/a b.pdf` | `'/home/me/a b.pdf'` |
| Directory | directories | `/home/me/Documents` | `'/home/me/Documents'` |
| Composed command | composition | `readlink -f '/tmp/a b'` | `readlink -f '/tmp/a b'` |
| Typed shell command | user text | `ps aux \| grep firefox` | `ps aux \| grep firefox` |

# Direct Execution

Some selected values do something useful when executed directly even though
their inserted shell text is not itself a useful command.

For example, a file should be inserted as:

```sh
'/home/me/paper.pdf'
```

when it is used as an argument, but pressing `[ent]` on that file should open it,
not try to execute the file path as a program.

Direct execution behavior:

1. Files open with the configured file opener, e.g. shell-equivalent
   `xdg-open {}`.
2. Directories open with the configured directory opener, e.g. shell-equivalent
   `xdg-open {}`.
3. Executables: execute the value itself.

Direct execution behavior is only used when the selected value is the whole
action, with no queued values and no surrounding command text. It does not affect
slot insertion or argument insertion. A file queued before `evince` is still
inserted as a quoted path, not as `xdg-open 'path'`.

# Modes

`fzlaunch` has two input modes.

## Search mode

This is the startup mode.

Typing changes the search query. Results update live. Text input resets the
selection to the top match.

`[up]` and `[down]` move the selected result. The preview updates live.

In search mode, `[tab]` and `[ent]` first resolve the current value:

1. If there are no matches, the current value is the search-box text as typed.
2. If the selected result's editable text is a proper prefix of the search-box
   text, the current value is the search-box text as typed.
3. Otherwise, the current value is the selected result.

Examples:

```text
fir[ent]
```

`firefox` is selected, and `firefox` is not a prefix of `fir`, so this executes
the selected `firefox` value.

```text
firefox --private-window[ent]
```

The selected value is `firefox`, and its editable text is a proper prefix of the
search-box text. The typed buffer wins, so this executes:

```sh
firefox --private-window
```

```text
ps aux | grep firefox[ent]
```

If this matches a strangely named file, the selected file would normally win. To
force direct shell entry, use initial `[backtick]`:

```text
[backtick]ps aux | grep firefox[ent]
```

## Edit mode

`[backtick]` enters edit mode.

Normally, `[backtick]`:

1. Copies the selected value's editable text into the search box.
2. Preserves how that value is inserted into shell text.
3. Clears/ignores the result list.

Subsequent typing edits that value directly. It does not produce new matches.

Example:

```text
;ddocres[backtick]/2024-polynomial-interpolation.pdf
```

If `;ddocres` selects `/home/me/Documents/research`, the search box becomes:

```text
/home/me/Documents/research/2024-polynomial-interpolation.pdf
```

The edited directory is still inserted as a quoted path:

```sh
'/home/me/Documents/research/2024-polynomial-interpolation.pdf'
```

The one explicit exception is initial `[backtick]`: if the search box is empty
when `[backtick]` is pressed, it enters edit mode with an empty buffer and does
not copy the selected result.

Example:

```text
[backtick]ps aux | grep firefox[ent]
```

executes the typed shell command.

# Slots

A slot is the exact substring:

```text
{}
```

Only that exact substring is a slot.

Examples:

```text
{}
```

contains one slot.

```text
{{}}
```

also contains one slot. The slot is the inner `{}`, so filling it with `x`
produces:

```text
{x}
```

```text
{file}
```

contains no slots.

When a value is inserted into a slot, it uses the same shell text it would use as
an argument. Command text is inserted verbatim. Paths are inserted quoted.

Any remaining slots stay in the command text.

Typing `{` in search mode first resolves the current value, enters edit mode with
that value, then inserts `{`. This makes common command composition short:

```text
mv {} {}
```

The first `{` resolves `mv ` as the current value, enters edit mode with that
typed buffer, and then appends `{`.

# Queue and composition

The queue is FIFO.

`[tab]` resolves the current value, composes it with queued values if possible,
then queues the result.

`[ent]` resolves the current value, composes it with queued values if possible,
then executes if the resulting command is complete. If the result still has open
slots, `[ent]` behaves like `[tab]` and leaves the incomplete value queued.

The composition rule is:

1. If the current value has slots, queued values are slotted into the current
   value from oldest to newest.
2. If the current value has no slots, but the oldest queued value has slots, the
   current value is slotted into that queued value.
3. If neither side has slots, queued values become command-line arguments to the
   current value when `[ent]` is pressed.

This rule means that when both the queued value and the new value have slots, the
queued value is inserted into the new value.

Argument insertion uses the same shell text as slot insertion. Command text is
appended verbatim. Paths are appended quoted. Direct execution behavior is not
used for queued arguments.

Slot filling consumes queued values. If the current value runs out of slots
before the queue is empty, the remaining queued values stay queued. On `[ent]`,
those remaining queued values are appended as command-line arguments to the
completed current value.

Example:

```text
readlink -f {}[tab]nvim $({})[tab]
```

After the first `[tab]`, the queue contains:

```sh
readlink -f {}
```

The new value is:

```sh
nvim $({})
```

Both values have slots, so the queued value is inserted into the new value:

```sh
nvim $(readlink -f {})
```

Example with more queued values than slots:

```text
a[tab]b[tab]cmd {}[ent]
```

If `a` and `b` select command text, final command:

```sh
cmd a b
```

# Examples

## Run an application

```text
fir[ent]
```

If the best match for `fir` is `firefox`, this executes:

```sh
firefox
```

## Open a file directly

```text
docrespoly[ent]
```

If the best match is:

```text
/home/me/Documents/research/2024-polynomial-interpolation.pdf
```

then pressing `[ent]` opens that file, for example with:

```sh
xdg-open '/home/me/Documents/research/2024-polynomial-interpolation.pdf'
```

## Open a selected file with a chosen program

```text
docrespoly[tab]evin[ent]
```

Step by step:

1. `docrespoly` selects the PDF.
2. `[tab]` queues the PDF so it will be inserted as a quoted path.
3. `evin` selects `evince`.
4. `[ent]` executes the current value with the queue as arguments.

Final command:

```sh
evince '/home/me/Documents/research/2024-polynomial-interpolation.pdf'
```

## Display paths but preview escaped shell text

Suppose a selected file has this editable text:

```text
/home/me/a'b.txt
```

After `[backtick]`, the search box shows the editable path. It does not show the
shell-escaped form.

When the value is queued or previewed as final shell text, it is shown escaped:

```sh
'/home/me/a'\''b.txt'
```

## Add arguments to a command by typing past the selected prefix

```text
firefox --private-window[ent]
```

The selected value `firefox` is a proper prefix of the typed buffer, so the typed
buffer wins and executes as typed:

```sh
firefox --private-window
```

The same resolution is used for `[tab]`:

```text
firefox --private-window[tab]
```

queues the command as typed:

```sh
firefox --private-window
```

## Type a shell command directly

```text
[backtick]ps aux | grep firefox[ent]
```

Initial `[backtick]` enters edit mode with an empty buffer. The command
executes as typed:

```sh
ps aux | grep firefox
```

## Move and rename a file with slots

Suppose:

1. `2024pdf` selects `/home/me/Downloads/2024-8234.pdf`.
2. `secm` selects the command `securemove`.
3. `;ddocres` selects `/home/me/Documents/research`.

Then:

```text
2024pdf[tab]secm[backtick] {} {}[tab];ddocres[backtick]/2024-polynomial-interpolation.pdf[ent]
```

Step by step:

1. `2024pdf[tab]` queues the downloaded PDF so it will be inserted as a quoted
   path.
2. `secm[backtick]` seeds the buffer with `securemove`.
3. ` {} {}` edits it into a command with two slots.
4. `[tab]` fills the first slot from the queue and queues:

```sh
securemove '/home/me/Downloads/2024-8234.pdf' {}
```

5. `;ddocres[backtick]` enters edit mode with the selected directory.
6. `/2024-polynomial-interpolation.pdf` edits that value.
7. `[ent]` fills the remaining slot and executes:

```sh
securemove '/home/me/Downloads/2024-8234.pdf' '/home/me/Documents/research/2024-polynomial-interpolation.pdf'
```

## Compose nested shell fragments

Suppose `;ffilename` selects:

```text
/home/me/link to paper.pdf
```

Then:

```text
;ffilename[tab]readlink -f {}[tab]nvim $({})[ent]
```

Step by step:

1. The file is queued so it will be inserted as a quoted path.
2. `readlink -f {` resolves the typed buffer `readlink -f `, enters
   edit mode, and then appends `{`.
3. `[tab]` fills the slot with the queued file and queues:

```sh
readlink -f '/home/me/link to paper.pdf'
```

4. `nvim $({})[ent]` inserts that queued shell fragment into the new command and
   executes:

```sh
nvim $(readlink -f '/home/me/link to paper.pdf')
```

## Preserve slots through composition

```text
readlink -f {}[tab]nvim $({})[tab]
```

queues:

```sh
nvim $(readlink -f {})
```

The remaining slot can be filled later:

```text
;ffilename[ent]
```

Final command:

```sh
nvim $(readlink -f '/home/me/link to paper.pdf')
```

## Bracket a slotted value

```text
foo[tab]echo {{}}[ent]
```

If `foo` selects `bar`, final command:

```sh
echo {bar}
```

# Searchable Values

The built-in searchable values are:

1. Files
2. Directories
3. Executables

Search text can include a selector:

1. Files: `;f`
2. Directories: `;d`
3. Executables: `;c`

Examples:

1. `;fpaper` searches files for `paper`.
2. `;ddocres` searches directories for `docres`.
3. `;creadl` searches executables for `readl`.

# Sorting

Results are sorted by fuzzy match quality. The intended value should usually be
reachable with very few typed characters.

# Key bindings

| Key | Behavior |
| --- | --- |
| Text input | Updates search in search mode; edits buffer in edit mode |
| `[up]` / `[down]` | Move result selection |
| `[backtick]` | Enter edit mode; normally seed from selected value |
| Initial `[backtick]` | Enter edit mode with an empty buffer that executes as typed |
| `{` in search mode | Resolve current value, enter edit mode, then insert `{` |
| `[tab]` | Resolve, compose, and queue |
| `[ent]` | Resolve, compose, and execute if complete |
| `ctrl-P` | Show/hide preview |
