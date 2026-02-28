# Caching mechanism

## mbox tree

- mbox name
- list of emails

## Email

- id
- reply_to
- date
- subect
- cc
- to
- body
- ordered Email list

## First opening

At the very beginning there's no cache, so we need to go through all the emails
and to build a dependency tree which can be easily fetched at the next start
without the need of reading all emails at once.

This requires a certain amount of operations:

- looping inside mbox and find email with ID
- open email file and extract all the information

We need to speedup the process by saving all information we already need at the
beginning. These might be:

- id
- reply_to
- date
- subject
- cc
- to
- read (flat telling if email has been read already)
- body size

The email's `body` would be removed from this equation, since it increases cache
size, slowing down deserialization process. `body` can be loaded at runtime.

## Caching algorithm

Email files might be read in a random order, so we can't really create a table
of replies for each email in `O(n)`, where `n` is the number of emails, unless
we reserve a slot for each parent we didn't reach yet.

For instance, let's suppose that we would like to obtain the following tree:

```text
A -- B -- C
 `-- D -- E -- F
           `-- G
```

Unfortunately, we might receive `E` before `A` and `D`, so it becomes hard to
guess where we can find the parents:

```text
? -- B -- C
 `-- ? -- E -- F
           `-- G
```

By using a Hash we can store parents IDs we are searching for and the list of
children which are searching for it. We will probably need a new hash also to
store the list of emails we already seen. In this way, every search operation
will cost `O(1)`.

- `Hs` associates `reply_to` IDs to a list of emails which are searching for it
- `Hm` associates `id` to its message

We can define a parent as an email with empty `reply_to`, hence:

- `Lp` is the list of parents

The insertion algorithm will look like this:

1. we read a new message `Mi`
2. we extract `reply_to` and if
   - it's empty, we add it to `Lp`
   - it's non empty
     - we extract `Hm[reply_to]`
       - if it's empty, we add it to `Hs[reply_to]`
       - if it's not empty, we add `Mi` to `Hm[reply_to]` children
3. we extract `id` from `Mi` and if `Hs[id]`
   - is not empty, we set `Hs[id]` as children of `Mi`
   - is empty, we don't do anything
4. we add `Mi` to `Hm[id]`

This has to be repeated for all the messages we read.
