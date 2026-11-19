# Custom Locale

[简体中文](custom-locale.md) | [English](custom-locale.en.md)

This document explains how to customize UI translations or add new languages in StripchatRecorder.

Translations are loaded from **JSON files on disk** rather than being compiled into the binary. The program creates default locale files on first run, which you can edit freely — your changes are never overwritten by updates.

---

## Directory Structure

```
<exe_dir>/
└── locale/
    ├── .builtin-state.json         # Managed state for built-in files (content fingerprints, maintained by the program)
    ├── app/                        # Main application translations
    │   ├── zh-CN.json              # Simplified Chinese (default)
    │   └── en-US.json              # English
    ├── log/                        # Backend log translations (affects log output language)
    │   ├── zh-CN.json
    │   └── en-US.json
    └── modules/                    # Module translations
        ├── filter_short/
        │   ├── zh-CN.json
        │   └── en-US.json
        ├── contact_sheet/
        │   ├── zh-CN.json
        │   └── en-US.json
        ├── notify_discord/
        │   ├── zh-CN.json
        │   └── en-US.json
        ├── notify_telegram/
        │   ├── zh-CN.json
        │   └── en-US.json
        ├── cleanup/
        │   ├── zh-CN.json
        │   └── en-US.json
        └── __builtin__/            # Built-in DAG node translations (recording_input, unpack)
            ├── zh-CN.json
            └── en-US.json
```

> **Note:** Files are created automatically on first run. **Built-in locale files** (`app/`, `log/`, `modules/<builtin_id>/`) are fingerprint-versioned: the program records a content fingerprint of every file it writes in `locale/.builtin-state.json` — a built-in file you have **not** modified is refreshed automatically when an upgrade ships new default text; a file you **have** modified is recognized as customized (fingerprint mismatch → marked `"custom"`) and is **never overwritten**. Files you create yourself (third-party module folders, custom locale codes) are unmanaged and likewise never overwritten.
>
> Exception: if a built-in locale file exists but fails JSON validation, the program backs it up first (`xxx.json.bak`, timestamped when the name is taken) and then rebuilds it with default content. Custom locale files only log a warning on validation failure and are not rebuilt.
>
> On the first run of this mechanism after upgrading from an older version: existing files without a fingerprint record are claimed when identical to the default; when they differ it is impossible to tell "stale default" from "user customization", so the file is backed up to `.bak` and refreshed to the new default — recover any manual edits from the backup.

---

## Modifying Existing Translations

Simply edit the JSON file for the desired language under `locale/app/` or `locale/modules/<module_id>/`.

The frontend fetches `/api/locale/{code}` on startup and on every language switch, so your changes take effect immediately after **reloading the page** — no rebuild required.

---

## Adding a New Language

### 1. Create the locale file

Create a new file under `locale/app/` using a BCP 47 language tag as the filename, e.g. `ja-JP.json`.

Copy `en-US.json` as a template and translate all string values (keep all keys and `{variable}` placeholders unchanged). Set the `languageName` field to the language's own native name:

```json
{
  "languageName": "日本語",
  "nav": {
    "streamers": "配信者",
    ...
  },
  ...
}
```

**That's it — no code changes required.** After restarting the program, `/api/locales` automatically includes the new language and both the Settings page and the setup wizard will display it in the language list.

> **Fallback language:** Simplified Chinese. Entries your locale file doesn't translate (including text added in later versions) are shown in Simplified Chinese instead of the raw key; this applies to the UI, backend logs and module translations. Entries missing from English are shown in Simplified Chinese too.

---

## Module Translations

Each module has its own folder under `locale/modules/<module_id>/`. Place a `{locale_code}.json` file there with the following structure:

```json
{
  "name": "My Module",
  "description": "Module description",
  "params": {
    "param_key": { "label": "Param label" }
  }
}
```

All fields are optional — any omitted field falls back to the module's own `--describe` JSON value.

> **Priority:** Server-side locale JSON overrides `--describe` i18n → `--describe` i18n overrides the original default value.

For third-party modules (not in the built-in list), simply create a folder with the module's `id` under `locale/modules/` and add the JSON files — the system discovers them automatically. When there's no file for the current language, the folder's `zh-CN.json` is used.

### Built-in DAG node translations (`__builtin__`)

The `__builtin__` module locale file uses a **per-node nested structure**, unlike the flat structure of regular modules:

```json
{
  "recording_input": {
    "name": "Recording Input",
    "description": "Virtual recording input node — always the pipeline start point"
  },
  "unpack": {
    "name": "Unpack Media Bundle",
    "description": "Split a media bundle into a video file (port 0) and an image file (port 1)"
  }
}
```

---

## Translation Key Reference

| Top-level key    | Description                                                   |
| ---------------- | ------------------------------------------------------------- |
| `nav`            | Sidebar navigation labels                                     |
| `common`         | Shared button text (confirm, cancel…)                        |
| `notify`         | System notifications and dialogs (disconnect, missing files…) |
| `home`           | Streamers list page                                           |
| `streamerCard`   | Streamer card component                                       |
| `addStreamer`    | Add streamer dialog                                           |
| `recordings`     | Recordings page                                               |
| `postprocess`    | Post-processing pipeline page                                 |
| `finder`         | Streamer finder page                                          |
| `statusPage`     | Service status page                                           |
| `usagePage`      | Resource usage page                                           |
| `settings`       | Settings page                                                 |
| `usePostprocess` | Post-processing task status messages                          |
| `relay`          | Relay streams page                                            |
| `setup`          | First-launch setup wizard                                     |
| `login`          | Login page (password input, initial password setup)           |
| `notifications`  | In-app notification panel                                     |
| `dirBrowser`     | Directory picker dialog                                       |
| `streamers`      | Streamer event messages (rename notifications, API errors…)  |
| `stripchat`      | StripChat browse page                                         |
| `community`      | Community module marketplace page                             |
| `about`          | About page (version info, update check)                       |
| `ffmpeg`         | FFmpeg download progress and preference wizard                |

Interpolation variables use the `{variableName}` format. Always keep placeholders as-is when translating:

```json
"reconnected": "Reconnected to server, reloading in {n} second(s)…"
```
