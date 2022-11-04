# fzlaunch
System one-stop-shop: A modular application launcher, file finder, command runner, etc. using fzf

## General overview

`fzlaunch` can launch applications, open files, or build commands to execute, all using `fzf`.
Its basic features are:
* Load entries from modules — these can be executables in `$PATH`, files in the user home, or other user-defined items
* Each entry also has a default action. For example, executables are just run in a shell, while
files are opened with `xdg-open`.
* Besides using the default action, the user can build up commands using the entries.

To run `fzlaunch`, just run it in your favorite temrinal: for example, `kitty fzlaunch`.
It's best to bind this command to a keyboard shortcut.

## Entries

Entries are provided by modules. For more in-depth information, see the
[modules documentation](https://github.com/tmarkov/fzlaunch/blob/main/modules/modules/modules.md).
They typically have a type, which is dentored with a character. The default types are
`f` for regular file, `d` for directory, `l` for symlink, and `c` for executable (command),
however, user-defined modules can have other types. The default actions mentioned above
are actually associated with file tyoes.

On the `fzf` window, you'll see lines like `;type name`, which you can filter by and select.
For example:
```
;c firefox
;d /home/you
;f /home/you/file.txt
;q custom entry from user module
```
Note the semi-colon before the type: this'll allow you to easily filter by it.

Note that these lines contain a `type` and a `name` field. An entry also has a hidden `item` field,
which is usually the same as the name. However, they can differ. For example, the name can be
an application name, while the item — a path to its `.desktop` file.

Besides the above, entries contain preview information for `fzf`.

## Basic workflow

1. When `fzlaunch` is opened, it loads the entries provided by its modules.
    1. While most modules would want to load their entries at launch,
some can have deferred actionation and get triggered on a keyword.
        * For example, most users wouldn't want entries for hidden files to
appear on startup. Thus, the hidden files are rpovided by a separate
module that's triggered if the user writes a ` . ` in the `fzf` prompt.
        * The keywords need to have whitespace on both sides to trigger.
    2. `fzlaunch` stores a history of previously sleected entries, and will also show
    them on launch based on frequency.
2. If the user simply selects an entry and hits `enter`, the default action for that entry
will be executed.
3. If instead, they hit `tab` or \`, `fzlaunch` will enter a command builder mode instead.
4. If the user hits `enter`, but selects no entry (nothing matched the query),
`fzlaunch` will execute the query text in a shell.

## Command builder workflow

The command builder workflow is entered if the user uses `tab` or \` on the first entry. The command builder mode would
best be explained by examples:

### Move and rename file

1. Select a file entry and hit `tab`
2. Write `mv {} {d}` in the query line, and hit `tab`.
    * This will load a `mv` command, and the `{...}` field in it will be substituted.
    * The first field — in this case `{}` — will be substituted with the file selected before.
    * The `d` in the second `{...}` field dentoes the type on entry to go there.
3. You'll see a new fzf window with a list of directories. Find a target directory and hit `
    * Note, if you simply want to move the file without renaming it, you can hit `enter` instead.
4. \` (above `tab` in a normal querty keyboard) will load the path of the directory in the query line. Add a new file name to the path, and hit `enter`.
    * Note that this is no logner a directory. The type `d` is not enforcedl it's simply used to detemine
    which entries to show to make filtering easier.

### `echo $(which firefox)`

There are multiple ways to build this command. We'll see two.

Here's the first:
1. Select `echo`, hit \`, and append ` $({c})` (or simply write `echo $({c})`). Hit `tab`.
2. Select `firefox` and hit `enter`.

Another would be:
1. Select `firefox`, `tab`
2. Select `which {}`, `tab`
3. Select `$({})`, `tab`
4. Select `echo {}`, `enter`

### Some rules

The above gives some intuition, but here are some rules `fzlaunch` will follow:
1. Pressing `tab` tells `fzlaunch` there's more to build. But it'll only keep one item.
2. Thus, if the item has `{...}` fields, at most one (the first) will be substituted with what's been built so far.
Any other `{...}` fields will ask you for items.
3. `{}` means entry of any type should be shown when substituting it. There can be multiple letters betweeb the brackets,
in which case entries from any of the types will be shown.
4. When selectign an item, either the `item` field of the entry, or the query text will be used. The query text is usef if:
    * either there was not entry selected (note, `fzf` will always select an entry if at least one matches the query);
    * or the `item` field of the selected entry is a substring of the `query` text
    (so, you can prepend envoronment variables or append options and `{...}` fields).
5. Hitting \` (above `tab` in the normal querty keyboard) will put the entry `item` field in the query.
6. Hitting `enter` will try to execute the command built so far if there aren't any `{...}` field in it; otherwise it'll act like `tab`.
7. Hitting `escape` will go a step back in the building process.
