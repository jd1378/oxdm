# oxdm recipes

All examples use `--format json`. Parse stdout line by line as JSON; on a
non-zero exit, stderr holds one JSON error object.

## 1. Download one file and get its path

```bash
oxdm --format json add "https://example.com/file.zip" -o ./downloads/ --wait --timeout 5m
echo "exit=$?"
```

The terminal line:

```json
{"type":"completed","id":"4e23b477-…","path":"/work/downloads/file.zip","already_complete":false}
```

Use `path` rather than building one yourself: oxdm may have numbered the
name if another download already uses it.

## 2. Keep following a long download

`--timeout` ends the waiting, not the download. On exit 124, follow it
again with the `id` from the `added` line:

```bash
oxdm --format json wait 4e23b477 --timeout 5m
```

## 3. Resume after a failure

```bash
oxdm --format json add "https://example.com/big.iso" --wait --timeout 5m
# exit 3, and the failed line says:
# {"type":"failed","id":"46c01576-…","phase":"failed",
#  "error":{"kind":"network","message":"network error: connection failed","retryable":true}}

oxdm --format json resume 46c01576 --wait --timeout 5m
```

The download continues from the bytes it already has. Running the same
`add` command again does the same thing: the existing download is found
and resumed rather than added twice.

Every failed download at once:

```bash
oxdm --format json resume --failed --wait --timeout 10m
```

## 4. A batch

```bash
# urls.txt: one URL per line; '#' and '//' lines are skipped
oxdm --format json add -i urls.txt -o ./downloads/ --wait --timeout 10m
```

One `added` (or `rejected`) line per URL, then progress, then one terminal
line per download. Match lines by `id`. The exit code comes from the first
URL in the list that did not finish.

## 5. Queue now, follow later

```bash
oxdm --format json add "https://example.com/a.bin" "https://example.com/b.bin"
# ... other work ...
oxdm --format json list --category agent --state active --state queued
oxdm --format json wait <id-a> <id-b> --timeout 5m
```

## 6. Authenticated download (secrets from stdin, never argv)

```bash
printf 'Authorization: Bearer %s\n' "$API_TOKEN" |
  oxdm --format json add "https://api.example.com/artifact" -H @- --wait --timeout 5m
```

`Cookie` and `Authorization` (Basic or Bearer) are stored encrypted by
oxdm. Other headers are plain: `-H "Accept: application/octet-stream"`.

## 7. Verify against a known checksum

```bash
oxdm --format json add "https://example.com/file.zip" \
    --checksum "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08" \
    --wait --timeout 5m
```

The digest can also be base64:
`--checksum "sha256:base64:n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg="`.
Give `--checksum` with one URL per command.

A mismatch ends with `failed` (`"kind":"conflict"`) and exit 4. Running the
same `add --checksum ...` for a download that already finished hashes the
saved file instead of downloading it again, so it also answers "is the
file I already have the right one?".

The file is complete but wrong; downloading it again is the user's call:

```bash
oxdm --format json restart <ID> --delete-file --wait --timeout 5m
```

## 8. Probe before committing

```bash
oxdm --format json probe "https://example.com/file.zip"
```

Use `size` to check disk space and `resumable` to know whether an
interruption will cost the whole file.

## 9. A retry loop in bash

```bash
url="https://example.com/file.zip"
id=""
for attempt in 1 2 3 4 5; do
  if [ -z "$id" ]; then
    out=$(oxdm --format json add "$url" -o ./downloads/ --wait --timeout 10m)
  else
    out=$(oxdm --format json resume "$id" --wait --timeout 10m)
  fi
  code=$?
  id=$(printf '%s\n' "$out" | jq -r 'select(.type=="added" or .type=="resumed") | .id' | head -n1)
  case $code in
    0)   printf '%s\n' "$out" | jq -r 'select(.type=="completed") | .path'; break ;;
    124) oxdm --format json wait "$id" --timeout 10m && break ;;      # still going
    3)   sleep $((attempt * 5)) ;;                                    # network: resume
    *)   echo "not retryable (exit $code)" >&2; break ;;             # 2, 4, 5, 6, 7
  esac
done
```

## 10. What have I downloaded?

```bash
oxdm --format json list --category agent --state completed
```

Everything added through `oxdm add` is filed under the Agent category,
finished or not (unless the user deleted that category).

## 11. Clean up after yourself

```bash
oxdm --format json remove <ID>                  # off the list, file kept
oxdm --format json remove <ID> --delete-file    # file moved to the trash too
```

Only remove what the user asked you to download, and only when they
asked for it to go.
