# oxdm recipes

The basic add, wait and resume loop is in `SKILL.md`. All examples use
`--format json`; parse stdout line by line, match lines by `id`.

## 1. A batch

```bash
# urls.txt: one URL per line; '#' and '//' lines are skipped
oxdm --format json add -i urls.txt -o ./downloads/ --wait --timeout 10m
```

One `added` (or `rejected`) line per URL, then progress, then one
terminal line per download. The exit code comes from the first URL in
the list that did not finish.

## 2. Queue now, follow later

```bash
oxdm --format json add "https://example.com/a.bin" "https://example.com/b.bin"
# ... other work ...
oxdm --format json list --category agent --state active --state queued
oxdm --format json wait <id-a> <id-b> --timeout 5m
```

## 3. Authenticated download (secrets from stdin, never argv)

```bash
printf 'Authorization: Bearer %s\n' "$API_TOKEN" |
  oxdm --format json add "https://api.example.com/artifact" -H @- --wait --timeout 5m
```

`Cookie` and `Authorization` (Basic or Bearer) are stored encrypted by
oxdm. Other headers are plain: `-H "Accept: application/octet-stream"`.

## 4. Verify against a known checksum

```bash
oxdm --format json add "https://example.com/file.zip" \
    --checksum "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08" \
    --wait --timeout 5m
```

The digest can also be base64:
`--checksum "sha256:base64:n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg="`.

A mismatch ends with `failed` (`"kind":"conflict"`) and exit 4. For a
download that already finished, the same command hashes the saved file
instead of downloading it again, so it also answers "is the file I
already have the right one?". Downloading a wrong file again is the
user's call:

```bash
oxdm --format json restart <ID> --delete-file --wait --timeout 5m
```

## 5. A retry loop in bash

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
