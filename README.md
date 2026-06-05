# Photo Backup

`photo-backup` is a Rust tool that recursively scans a folder and uploads supported photos and videos to Google Photos.

It stores progress on disk so an interrupted backup can resume later.

## Features

- Recursive folder scanning
- Photo and video detection
- Unsupported file skipping with local logging
- Durable on-disk checkpoints for resume support
- Retry handling for transient upload failures
- Simple terminal command loop with:
  - `start`
  - `pause`
  - `resume`
  - `status`
  - `rescan`
  - `clean`
  - `quit`

## Requirements

- Rust toolchain
- A Google account
- A Google Cloud project with the Google Photos API enabled
- OAuth credentials for an installed/desktop application

## Before You Run

1. Create or choose a Google Cloud project.
2. Enable the Google Photos Library API.
3. Create OAuth credentials for an installed application.
4. Copy the OAuth client ID.
5. Copy the client secret too, if your OAuth client has one.

The first time you run the app, it will open a browser-based Google login flow.

## Build

From the repository root:

```bash
cargo test
```

That command downloads dependencies, builds the workspace, and runs the tests.

## Run

### Using environment variables

Set your Google OAuth credentials:

```bash
export GOOGLE_CLIENT_ID=YOUR_GOOGLE_OAUTH_CLIENT_ID
export GOOGLE_CLIENT_SECRET=YOUR_GOOGLE_OAUTH_CLIENT_SECRET
```

Then run the app and point it at the folder you want to back up:

```bash
cargo run -p photo-backup-cli -- /path/to/source-folder
```

### Passing credentials on the command line

You can also pass the credentials directly:

```bash
cargo run -p photo-backup-cli -- /path/to/source-folder \
  --state-dir /path/to/source-folder/.photo-backup-state \
  --client-id YOUR_GOOGLE_OAUTH_CLIENT_ID \
  --client-secret YOUR_GOOGLE_OAUTH_CLIENT_SECRET
```

If you do not pass `--state-dir`, the app uses a hidden `.photo-backup-state` folder inside the source directory.

## Step-by-Step

1. Clone the repository.
2. Open a terminal in the repository root.
3. Verify the project builds:

   ```bash
   cargo test
   ```

4. Set `GOOGLE_CLIENT_ID` and optionally `GOOGLE_CLIENT_SECRET`.
5. Choose the folder you want to back up.
6. Start the backup:

   ```bash
   cargo run -p photo-backup-cli -- /path/to/source-folder
   ```

7. When the prompt appears, use these commands:
   - `start` to begin uploading
   - `pause` to pause work
   - `resume` to continue after a pause
   - `status` to print progress as JSON
   - `rescan` to scan the source folder again
   - `clean` to wipe the current local backup state for that folder
   - `quit` to stop the program

## State Files

The app keeps local state in the source folder unless you choose a custom state directory.

Default state folder:

```text
/path/to/source-folder/.photo-backup-state
```

Files stored there:

- `manifest.json` - all discovered files
- `checkpoint.json` - resumable backup status
- `events.jsonl` - append-only event log
- `google_token.json` - cached OAuth token

## Supported Files

The tool currently treats these as supported:

- Images: `jpg`, `jpeg`, `png`, `gif`, `bmp`, `webp`, `heic`, `heif`, `tif`, `tiff`, `dng`, `raw`
- Videos: `mp4`, `mov`, `m4v`, `avi`, `mkv`, `3gp`, `3g2`, `mpg`, `mpeg`, `mts`, `m2ts`, `webm`

Other file types are skipped and recorded in the checkpoint.

## Resume Behavior

- The source folder is scanned recursively.
- Each discovered file is written into the local manifest and checkpoint.
- Upload progress is saved after scan and after each status change.
- If the app stops, you can run it again later and it will continue from the saved checkpoint.

## Troubleshooting

- If the app says a Google client ID is missing, set `GOOGLE_CLIENT_ID` or pass `--client-id`.
- If the browser does not open, copy the printed OAuth URL into your browser manually.
- If a file upload fails temporarily, the app retries automatically.
- If you want to refresh the file list, use `rescan`.

## Development

Useful commands:

```bash
cargo fmt --all
cargo test
cargo run -p photo-backup-cli -- /path/to/source-folder
```
