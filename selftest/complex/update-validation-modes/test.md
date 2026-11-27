# Update keeps testcases in `jsonschema` and `interactive` mode

Tests in this file validate that `update` leaves the body of testcases in the `jsonschema` and `interactive` validation modes as written, because it can only generate output expectations.

```scrut
$ alias scrut_update='$SCRUT_BIN update --match-markdown="*.mdtest"'
```

## Copy documents to update

```scrut
$ cp "$TESTDIR"/*.mdtest .
```

## Passing testcases keep their body

```scrut
$ scrut_update --replace --assume-yes passing.mdtest
Result: 1 document(s) of which 1 updated, 0 skipped and 0 unchanged
```

Only the outdated output expectation of the last testcase is updated:

```scrut
$ diff "$TESTDIR"/passing.mdtest passing.mdtest
20c20
< old
---
> new
[1]
```

## Failing testcases are left unchanged and do not abort the update

```scrut
$ scrut_update --replace --assume-yes failing.mdtest other.mdtest 2>&1 | grep -E 'WARN|^Result' | sed -E 's/^.*WARN [^ ]+ //'
testcase number 1 (line 4) failed, but testcases in `jsonschema` mode cannot be updated: it is left unchanged
testcase number 2 (line 13) failed, but testcases in `interactive` mode cannot be updated: it is left unchanged
Result: 2 document(s) of which 1 updated, 0 skipped and 1 unchanged
```

```scrut
$ diff "$TESTDIR"/failing.mdtest failing.mdtest
```

```scrut
$ diff "$TESTDIR"/other.mdtest other.mdtest
5c5
< old
---
> new
[1]
```
