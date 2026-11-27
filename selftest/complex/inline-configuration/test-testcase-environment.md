# Validate per-testcase environment configuration

Tests in this file validate that the `environment` configuration sets environment variables as intended.

## Environment variable is not set initially

```scrut
$ echo "Var is '${SOME_VAR}'"
Var is ''
```

## Environment variable is set

```scrut {environment: {"SOME_VAR": "some value"}}
$ echo "Var is '${SOME_VAR}'"
Var is 'some value'
```

## Environment variable will not be unset

```scrut
$ echo "Var is '${SOME_VAR}'"
Var is 'some value'
```

This is matching the behavior when `SOME_VAR` would be set within the shell expression by the user.

## Environment variable configured by a previous testcase can be changed

```scrut {environment: {"SOME_VAR": "other value"}}
$ echo "Var is '${SOME_VAR}'"
Var is 'other value'
```

## Environment variable exported by a previous testcase can be changed

```scrut
$ export SOME_VAR="exported value"
```

```scrut {environment: {"SOME_VAR": "configured value"}}
$ echo "Var is '${SOME_VAR}'"
Var is 'configured value'
```

## Unchanged configuration does not override an exported value

```scrut {environment: {"SOME_VAR": "configured value"}}
$ export SOME_VAR="exported again"
```

```scrut {environment: {"SOME_VAR": "configured value"}}
$ echo "Var is '${SOME_VAR}'"
Var is 'exported again'
```

Only configured values that differ from those of the previous testcase are applied over the inherited state. Otherwise a variable that Scrut sets for every testcase (e.g. `LANG`) could not be changed with `export`, and keep that value for later testcases.

## Configured values are applied verbatim

```scrut {environment: {"SOME_VAR": "it's $HOME `true`"}}
$ echo "Var is '${SOME_VAR}'"
Var is 'it's $HOME `true`'
```
