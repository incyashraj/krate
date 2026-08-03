# Deploying Krate Hub

Krate Hub is a single-binary HTTP server that stores published `.krate` bundles
content-addressed on disk and serves them back, so the loop

```
krate publish  ->  URL  ->  krate run <url>
```

works from anywhere. This guide gets it running behind a real public HTTPS URL.

Two paths below:

- **[A. fly.io](#a-flyio-recommended-one-command-host)** -- recommended. Free
  HTTPS + a public `https://<app>.fly.dev` URL, one command to deploy.
- **[B. Plain Docker host / VPS](#b-plain-docker-host--vps)** -- any box with
  Docker. You bring the HTTPS.

Both build the **same Docker image**. The build context is the **repo root**
(krate-hub is a workspace member and depends on workspace crates), so every
command below is run **from the repo root**, not from `crates/krate-hub/`.

> **Honest v1 note -- no auth.** This is a v1. There is **no authentication**:
> anyone who can reach the URL can publish to it. That is fine for a demo hub,
> but do not put anything you care about behind it, and don't hand the URL to
> people you wouldn't hand write access to. Hardening this is a small, known
> change -- see [Hardening: add an upload token](#hardening-add-an-upload-token).

---

## A. fly.io (recommended, one-command host)

### Prerequisites (YOURS to set up)

- A **fly.io account** and the `flyctl` CLI installed and logged in
  (`fly auth login`). *This is yours -- it needs your account and payment method
  on file even on the free-tier machines.*

The config lives at `crates/krate-hub/fly.toml`. Open it and change:

- `app` -- your app name (also sets the default `https://<app>.fly.dev` URL).
- `primary_region` -- a region near you (`iad`, `sjc`, `lhr`, `sin`, ...).
- `KRATE_HUB_PUBLIC_URL` -- **must match** your app name or custom domain. This
  origin is baked into every URL the hub returns; if it's wrong, the links it
  hands out won't resolve.

### Deploy

From the **repo root**:

```bash
# 1. Register the app on fly (uses the committed fly.toml; don't deploy yet).
fly launch --no-deploy --copy-config --dockerfile crates/krate-hub/Dockerfile

# 2. Create the persistent volume the config mounts at /data (once).
#    The name MUST match [mounts].source in fly.toml ("krate_data").
fly volumes create krate_data --size 1 --region <your-region>

# 3. Deploy.
fly deploy --dockerfile crates/krate-hub/Dockerfile
```

That's it. fly builds the image, boots a machine, terminates TLS for you, and
gives you `https://<app>.fly.dev`. `force_https = true` in the config redirects
any plain-HTTP request to HTTPS.

If you didn't set `KRATE_HUB_PUBLIC_URL` in `fly.toml` before deploying (or want
to change it without a redeploy), set it as a secret/env and restart:

```bash
fly secrets set KRATE_HUB_PUBLIC_URL=https://<app>.fly.dev
```

### Verify

```bash
curl https://<app>.fly.dev/health      # -> ok
```

### Publish to it

The client (`krate publish`) picks its hub from, in order: the `--hub` flag, then
the `KRATE_HUB_URL` env var, then a local default (`http://127.0.0.1:8787`). To
publish to your deployed hub:

```bash
krate publish path/to/app.krate --hub https://<app>.fly.dev
```

or set it once for the shell:

```bash
export KRATE_HUB_URL=https://<app>.fly.dev
krate publish path/to/app.krate
```

`publish` prints a URL like `https://<app>.fly.dev/a/<hash>`. Anyone can then:

```bash
krate run https://<app>.fly.dev/a/<hash>
```

### Custom domain (e.g. `hub.krate.tech` via Cloudflare)

*DNS is YOURS to configure.*

1. Tell fly about the hostname and let it issue a cert:

   ```bash
   fly certs add hub.krate.tech
   ```

   `fly certs show hub.krate.tech` prints the exact DNS records to add.

2. In **Cloudflare DNS**, add the records fly asks for (typically a `CNAME`
   pointing `hub.krate.tech` at `<app>.fly.dev`, plus an ACME `CNAME` for
   validation). Set the record to **DNS only (grey cloud), not proxied** --
   let fly terminate TLS, same as the existing `krate.tech` setup. If you proxy
   through Cloudflare (orange cloud) instead, turn on Full (strict) SSL so you
   don't double-terminate.

3. Point `KRATE_HUB_PUBLIC_URL` at the custom domain so returned links use it:

   ```bash
   fly secrets set KRATE_HUB_PUBLIC_URL=https://hub.krate.tech
   ```

4. Publish against it: `krate publish app.krate --hub https://hub.krate.tech`.

---

## B. Plain Docker host / VPS

Any host with Docker. **You provide HTTPS** -- either a reverse proxy
(Caddy/nginx/Traefik) terminating TLS in front, or Cloudflare's proxy. The hub
speaks plain HTTP on `8787`; it's proxy-ready because the public origin is set
independently via `KRATE_HUB_PUBLIC_URL`.

### Build (from the repo root)

```bash
docker build -f crates/krate-hub/Dockerfile -t krate-hub .
```

### Run

```bash
docker run -d \
  --name krate-hub \
  -p 8787:8787 \
  -v krate_data:/data \
  -e KRATE_HUB_PUBLIC_URL=https://hub.example.com \
  krate-hub
```

- `-p 8787:8787` -- publishes the container's port. Point your reverse proxy at
  `127.0.0.1:8787` (or bind it locally with `-p 127.0.0.1:8787:8787` and only
  expose the proxy).
- `-v krate_data:/data` -- named volume so published `.krate` bundles survive
  restarts and redeploys. `/data` is the container's `KRATE_HUB_DIR` and is
  declared a `VOLUME` in the image.
- `-e KRATE_HUB_PUBLIC_URL=...` -- **set this to your real public HTTPS origin.**
  It's the origin in every returned URL. The container already sets
  `KRATE_HUB_ADDR=0.0.0.0:8787` internally, so you don't pass that.

### Verify

```bash
curl http://127.0.0.1:8787/health      # -> ok
# through your proxy / domain:
curl https://hub.example.com/health    # -> ok
```

### TLS in front (example: Caddy)

Caddy gets you automatic Let's Encrypt certs with a two-line config:

```
hub.example.com {
    reverse_proxy 127.0.0.1:8787
}
```

Then set `KRATE_HUB_PUBLIC_URL=https://hub.example.com` on the container (above).

### Custom domain via Cloudflare

Same idea as fly: add a DNS record for `hub.example.com` pointing at your VPS.
If you use Cloudflare's proxy (orange cloud) for HTTPS, set SSL mode to
**Full (strict)** and terminate TLS on the origin (e.g. with Caddy). If you
terminate TLS yourself and only want DNS, use **grey cloud (DNS only)**.

### Publish to it

Same as fly -- point the client at your origin:

```bash
krate publish path/to/app.krate --hub https://hub.example.com
# or: export KRATE_HUB_URL=https://hub.example.com
```

---

## Environment variables (reference)

The hub reads exactly three (defaults in parentheses):

| Var | Default | What it does |
| --- | --- | --- |
| `KRATE_HUB_ADDR` | `127.0.0.1:8787` | Bind address. The container image sets this to `0.0.0.0:8787` so the port is reachable. |
| `KRATE_HUB_DIR` | `./hub-data` | Where bundles are stored, one file per content hash. The image sets this to `/data` (the volume). |
| `KRATE_HUB_PUBLIC_URL` | `http://127.0.0.1:8787` | The origin used to build returned URLs. **Set this to your real public HTTPS origin** in any real deployment. |

---

## Hardening: add an upload token

**Not built in v1** -- documented so the next step is obvious.

Right now `POST /publish` accepts any upload. The smallest hardening is a shared
upload token: require an `Authorization: Bearer <token>` header on publish,
compare it (constant-time) to a `KRATE_HUB_TOKEN` env var, and 401 on mismatch.
Fetches (`GET /a/<hash>`) stay open so `krate run <url>` still works for anyone
with a link.

Where it goes in `crates/krate-hub/src/main.rs`:

- Read the token in `Config` (alongside `addr` / `data_dir` / `public_base` in
  `main()`), from a new `KRATE_HUB_TOKEN` env var.
- Enforce it at the **top of `handle_publish`** (the `("POST", "/publish")` arm
  in `handle`), before reading the body: pull the `authorization` header with the
  existing `header(headers, "authorization")` helper, and `write_response(..., 401,
  ...)` if it's missing or wrong.

On the client side, `krate publish` would need to send the matching header
(that's a `crates/cli` change, out of scope here). Leaving auth out is a
deliberate v1 choice, not an oversight -- the content-addressed store has nothing
to overwrite, so the only thing a token buys you is keeping strangers from
filling your disk.
