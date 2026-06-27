# Overview

`fzlaunch` is a search-first, object-verb linux launcher focused on minimizing the keystrokes necessary to execute an action, and the amount of data the user needs to have memorized (e.g. commands).

# Startup

When started, fzlaunch opens a TUI with the following:
1. Search box: Text box with the cursor in it, for input of search terms. On top.
2. Status line: Text showing the current command composition. Below the search box, only visible if there is anything to show.
3. Result box: A box with a list of sorted matching items, with one item (first by default) selected. Left half below the status line. Best match on top.
4. Preview box: A box showing a preview of the currently selected item. The exact nature of the preview depends on the item. Hideable. Visible by default. Right half below the status line. If hidden, the result box expands to full width.

# User actions

1. Text input: Modify text in the search box. Results adjust live.
2. Up/down: Move selection in the results box. Preview adjusts live. Text input resets the selection to top match.
3. Tilde "`": Move selected item to the search box.
4. Tab: Enqueue current item
5. Enter: Execute current item
6. ctrl-P: Show/hide preview.

Search box is normally the selected item. However, if there are no matches, or if the selected item is a prefix of the search box text, it's the search box text.

# Workflow

The main idea is to allow the user to compose actions in an object-verb action model, and then execute them. This is done using the queueing mechanism. It's best to explain the workflow by examples.

Each example will contain a keystroke sequence, and then an explanation of the intended result.
We'll use the literal characters for alphanumeric inputs, [ent] for enter, _ for space (to make it obvious).

Let's start with the simplest use case: run an application or open a file:

1. fir[ent]: launch firefox (assuming that's the best match at "fir")
2. docrespoly[ent]: open `~/Documents/research/2024-polynomial-interpolation.pdf` (again, assuming this is the best match)
3. docrespoly[down][ent]: open the same file, if it's actually the second best match for "docrespoly".

But often, we want to execute something more complex than that: for example, we might want to provide command line arguments for a program. Suppose we don't have a default document reader set up, and so must specify that we want to open that document with evince. Then we use:

3. docrespoly[tab]evin[ent]: This works as follows:
  - "docrespoly": find `~/Documents/research/2024-polynomial-interpolation.pdf`
  - [tab]: queue the file, and start a new search. At this point, the status line shows "~/Documents/research/2024-polynomial-interpolation.pdf".
  - "evin": find evince
  - [ent]: execute the composed command, which is `evince ~/Documents/...`

Now lets consider what might happen if we want to run a command with two arguments. Suppose we have a `securemove` binary that acts like `mv`, but with additional checks. We've downloaded the polynomial interpolation paper in `~/Downlaods/2024-8234.pdf`, and want to move it to a better place with a different name.

4. 2024pdf[down][tab]secm[tilde]_{}_{}[tab];ddocres[tilde]/2024-polynomial-interpolation.pdf[ent]: This works as follows:
  - "2024pdf[down]": select the file, `~/Downloads/2024-8234.pdf`
  - [tab]: queue the file. Now the status line shows it, and we start a new search
  - "secm": select the `securemove` command.
  - "_{}_{}": now the search box reads "securemove {} {}". The "{}" are called "slots".
  - [tab]: Since the command has slots in it, we first do pop from the queue, and fill the flots. In this case, we only fill the first slot. Then we queue it. Now the status line is "securemove ~/Downloads/2024-8234.pdf {}"
  - ;ddocres: select the directory `~/Documents/research`, `;d` is a special matcher for directories
  - [tilde]: move it to the search box
  - "/2024-polynomial-interpolation.pdf" - append a filename to the path, since we also want to rename the file
  - [ent]: Normally, we would unqueue and place the queue items as command line arguments, then execute. But in this case, we see that the queueed item has unfilled slots, so we use the current item to fill the slots instead. Then we execute.

We can also do more complex things like:

5. ;ffilename[tab];creadl[tilde]_-f_{}[tab]nvim_$({})[ent]: executes `nvim $(readlink -f /path/to/ilename)`

`;f` matches files, `;d` - directories, and `;c` - executables

# Rules

1. We either have one queueed item with unfilled slots, or any number of queueed items
2. When we queue an item, if it has slots, we unqueue until we fill them, or the queue runs out. Then we queue.
3. When we execute an item, we take everything from the queue as command line arguments, unless the queue item has slots - then we slot the current item instead.
  - if we try to execute, and the queued item has more slots, we only fill one slot, then proceed as if [tab] was entered.

# Sources

We have three built-in sources:
- files
- directories
- executables

Each source has a "match character". For files this is "f", for directories - "d", and for executables, "c".

We have two types of sources:
- default: They always output items. Including `;x` increases the score for items output by a source with matching character "x"
- triggered: They are triggered when ";x", where "x" is a matching character, is entered into the search box. They receive what's in the search bot at the time of trigger, and then output items

For example:
A triggered source might be a calculator, with a match character "=".
Or a content indexer (e.g. `recoll`) with a match character "r".
They need to already have search terms in order to output any items.

# Sorting

We sort my a simply fuzzy matching algorithm, like `fzf` does, as we need it to be fast. This is regardless of source.

So, if we have a calculator source with a match character "=", and we enter "1 + 3;=", the calculator should output "1 + 3;= 4" so that the result is the top match.
