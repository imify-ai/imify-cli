# Open the platform in the browser

```console
$ imify browse
Opening https://platform.imify.ai in the browser.
```

`imify browse` starts the system's default browser with the API host: `https://platform.imify.ai`, unless `--api-url`, `IMIFY_API_URL`, the config file or a profile selects another (see [Configuration](configuration.md)). It needs no token and sends no request. On macOS it runs `open`, on Linux `xdg-open`, and returns without waiting for the browser.

```console
$ imify browse --profile staging
Opening https://staging.example.com in the browser.
```

## Without a browser

```console
$ imify browse --no-browser
https://platform.imify.ai
```

`--no-browser` prints the URL and opens nothing, for a machine without a browser or a script that needs the URL.

When the browser cannot be started, the command fails with `internal` and exit code 1. The hint names the URL:

```console
$ imify browse
error: Cannot open the browser: No such file or directory (os error 2).
hint: Open https://platform.imify.ai in a browser, or run `imify browse --no-browser` to print the URL.
```

## JSON output

```console
$ imify browse --json
{"opened":true,"url":"https://platform.imify.ai"}
```

| Field | Content |
|---|---|
| `url` | The API URL, without a trailing slash. |
| `opened` | `true` when the browser was started, `false` with `--no-browser`. |

Schema: [`browse.json`](../reference/schemas/browse.json).
