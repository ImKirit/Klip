# Klip Cloud

The server behind the cloud page of Klip. It stores clips of signed in users, either on its
own disk or in the Google Drive of each user, and shows them on a page made for the phone.
Sign in is Discord only. Node 22.13 or newer, no dependencies, one SQLite file.

## How it works

- **Phone and browser** sign in with Discord through the normal OAuth page. The session is a
  cookie that only holds a random id, the database stores its hash.
- **The PC** has no browser session. Klip asks for a short code, opens the confirm page, and
  once you confirm there it gets a token of its own. You can end it later on the settings page.
- **Uploads** go to `PUT /api/clips` with the video as body. The server keeps the clip on disk
  or streams it on to Google Drive (resumable upload). Playback always goes through the
  server, with Range support, so seeking works in every mobile browser.
- **Google Drive** uses the `drive.file` scope: the server sees only the files Klip created,
  nothing else in the Drive.

## Running it

```bash
npm test                                   # five tests, no network, fake Discord
KLIP_FAKE_DISCORD=1 node src/server.js     # local try out, signs in without Discord
```

`/auth/discord?as=42:Name` signs in as that fake account while `KLIP_FAKE_DISCORD=1` is set.
Never set it on a public server.

## Deploying

1. Create a Discord application, under OAuth2 add the redirect
   `https://<host>/<path>/auth/discord/callback` and note the client id and secret.
2. Optional: a Google Cloud OAuth client of type "Web application" with the Drive API
   enabled and the redirect `https://<host>/<path>/auth/google/callback`.
3. Copy this folder to `/opt/klip-cloud`, create the user and the data folder:
   ```bash
   useradd --system --no-create-home klip-cloud
   mkdir -p /var/lib/klip-cloud && chown klip-cloud: /var/lib/klip-cloud
   ```
4. Fill [`klip-cloud.env.example`](klip-cloud.env.example) into `/etc/klip-cloud.env`
   (`chmod 600`), install [`klip-cloud.service`](klip-cloud.service) and start it.
5. Forward the path in nginx, before any wider `location` of the same site:
   ```nginx
   location /klip/cloud/ {
       proxy_pass http://127.0.0.1:3160;
       proxy_set_header Host $host;
       proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
       proxy_request_buffering off;
       client_max_body_size 2100m;
       proxy_read_timeout 600s;
   }
   ```
   The server takes the full path, nginx must not strip it.
6. In Klip, Settings, Cloud: the server address is `https://<host>/<path>`.

## API

| Route | What it does |
|---|---|
| `GET /auth/discord` | Discord sign in for the web page |
| `GET /auth/google` | links a Google Drive to the signed in account |
| `POST /api/device` | starts a PC link, returns a code and the confirm page |
| `POST /api/device/token` | the PC asks whether the code was confirmed |
| `GET /api/me` | account, storage, sessions |
| `PUT /api/me/storage` | switch between server disk and Google Drive |
| `GET /api/clips` | all clips of the account, newest first |
| `PUT /api/clips` | upload a clip, headers carry the metadata |
| `GET /api/clips/:id/video` | the video, with Range |
| `GET /api/clips/:id/thumb` | the picture |
| `DELETE /api/clips/:id` | removes a clip everywhere |

Requests from the web page carry the cookie and the header `X-Klip-Web: 1`, which a foreign
page cannot send without permission. The PC sends `Authorization: Bearer <token>`.
