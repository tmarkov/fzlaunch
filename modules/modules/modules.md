# fzlaunch modules

`fzlaunch` is a modular launcher and system entry point.
It gets potential entries -- applciations, actions,
files, etc. -- from modules.

Each module is a program or a script that outputs
possible entries in a specific format.
There are two types of modules:
* No-query modules: they take no inputs, and
output entries to the standard output. No-query
modules can be ran when starting `fzlaunch`.
* Query modules: they return entries based on a query,
and must be manually triggered once a query has been
entered.

# Module output

Each `fzlaunch` module needs to output its entries
in the following format, one per line:

```
preview1 @@ item1 @@ ;type1 name1
preview1 @@ item1 @@ ;type1 name1
```

Each line contains 4 fields; `preview`, `item`, and `type` are separated by
` @@ ` (configurable), while `type` and `name` -- with a space ` `.

Fields `name` and `type` are searchable and shown in `fzf`.
* Field `name` contains an item name to be displayed and to search in.
* Field `;type` contains the type of the entry. It is used by `fzlaunch`
to determine what default action to execute, but is also searchable.
Types include:
  * `;d` for a directory,
  * `;f` for a file (in general)
  * `;a` for application (i.e. a .desktop file)
  * `;c` for a shell command (i.e. an executable file in `$PATH`)
  * a module can use any other character after a `;` for custom types
  * Note that a .desktop or execurable file can be marked as `;f`,
in which case it'll be treated as a regular file.
* `item` contains the actual item. It's not displayed or searchable,
and is often the same as `name`. But, for example, `name` can be the name
of an application, while `item` -- the path to its `.desktop` file.
* `preview` contains a preview command that `fzf` can use.
It should take `item` as its parameter.
* Fields containing ` @@ ` will cause problems. If necessary, change the separator.
Don't use characters that would be interpreted as regex.

# Module input

Non-query modules have no input.
Query modules will receive the query as the first command line argument.

