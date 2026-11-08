# dl-cn.stellarismod.com

The launcher's mirror for mainland China: nginx on a Hong Kong (CN2) server that caches the two Cloudflare R2 domains. The launcher asks it
and R2 at once and takes the first answer that verifies (`selfupdate::MIRRORS`, `pluginindex::MIRROR_INDEXES`).

| Path | Comes from |
|---|---|
| `/launcher/...` | `https://dl.stellarismod.com/launcher/...` (the launcher's releases) |
| `/plugins/...` | `https://plugins.stellarismod.com/...` (the plugins) |

Nothing else is passed on (404), and only GET/HEAD. Everything served is signed, or checked against a signed SHA-256, by the launcher, so
the server is trusted with nothing: a broken or hostile copy is refused and the launcher goes on to R2, then GitHub. How long a file is
cached follows R2's `Cache-Control`; when R2 cannot be reached, what is cached is served stale.

## The server (2026-11-08)

Ubuntu 26.04, nginx 1.28 from apt, certbot (webroot), ufw allowing only the SSH port, 80 and 443. SSH accepts keys only
(`/etc/ssh/sshd_config.d/00-stellarismod.conf`). DNS: `dl-cn` A record, **DNS only** (a proxied record would send mainland users through
Cloudflare's overseas nodes, which is what this server avoids).

## Setting it up again

```bash
apt-get install -y nginx certbot ufw
ufw allow <ssh port>/tcp && ufw allow 80/tcp && ufw allow 443/tcp && ufw --force enable
mkdir -p /var/www/certbot /etc/nginx/snippets /var/cache/nginx/stl-mirror && chown www-data: /var/cache/nginx/stl-mirror
rm -f /etc/nginx/sites-enabled/default
```

1. Put only the port-80 server block of `dl-cn.conf` in `/etc/nginx/conf.d/dl-cn.conf`, reload nginx, then get the certificate:
   `certbot certonly --webroot -w /var/www/certbot -d dl-cn.stellarismod.com --agree-tos --register-unsafely-without-email --deploy-hook 'systemctl reload nginx'`
2. Install `stl-mirror.locations` as `/etc/nginx/snippets/stl-mirror.locations` and `dl-cn.conf` as `/etc/nginx/conf.d/dl-cn.conf`;
   `nginx -t && systemctl reload nginx`.
3. Check: `/launcher/latest.json` twice gives `X-Cache-Status: MISS` then `HIT`, the same bytes as R2; `/` is 404.

The provider panel's password reset regenerates the SSH host keys, empties `authorized_keys` and sets `PubkeyAuthentication no` in
`sshd_config`: after one, key login has to be put back from the panel's console.
