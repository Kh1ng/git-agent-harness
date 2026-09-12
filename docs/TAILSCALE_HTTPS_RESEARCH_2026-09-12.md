# Tailscale names and HTTPS for GAH

Researched 2026-09-12 for [#943](https://github.com/Kh1ng/git-agent-harness/issues/943). This note uses current Tailscale, Caddy, and Apple documentation.

## Recommendation

Use MagicDNS plus Tailscale Serve for a new installation. Keep GAH on loopback HTTP, then run:

```bash
sudo tailscale set --hostname=hermesagent
sudo tailscale set --accept-dns=true
sudo tailscale serve --bg --https=443 http://127.0.0.1:3773
tailscale serve status
```

Serve terminates HTTPS and proxies to `http://127.0.0.1:3773`. Its background configuration survives the terminal session. Tailnet access rules still apply. [Tailscale Serve][serve], [Serve CLI][serve-cli].

The HTTPS URL is `https://hermesagent.<tailnet-name>.ts.net`, not `https://hermesagent`. Tailscale does not issue certificates for bare hostnames. The short name remains valid for HTTP when clients use MagicDNS. [Tailscale HTTPS][https], [MagicDNS][magicdns].

This changes acceptance criterion 2 in #943. Use the full `*.ts.net` name for HTTPS tests. Do not distribute Caddy's private root only to preserve a shorter URL.

If the host must retain Caddy, use Caddy's native Tailscale certificate support instead. Configure the full `*.ts.net` name as the Caddy site address. Caddy then gets the certificate from the local Tailscale daemon. A non-root Caddy service needs `TS_PERMIT_CERT_UID=caddy` in `/etc/default/tailscaled`. [Caddy certificates on Tailscale][caddy-tailscale].

## MagicDNS setup

Enable MagicDNS once on the tailnet's DNS page. New tailnets enable it by default. Set the central node's machine name to `hermesagent`. Machine names are unique, and renaming a machine changes its MagicDNS name. [MagicDNS][magicdns], [machine names][machine-names].

Each client must use the tailnet DNS configuration:

| Platform | Setting |
| --- | --- |
| Linux | Run `sudo tailscale set --accept-dns=true`. If DNS management conflicts occur, Tailscale recommends `systemd-resolved`. |
| macOS | Enable **Use Tailscale DNS settings** in Tailscale preferences. `tailscale set --accept-dns=true` is equivalent when CLI integration is available. |
| iOS | Leave **Use Tailscale DNS settings** enabled in the app. iOS has no shell command. The **Set Tailscale DNS** Shortcut can also turn this setting on. |

Tailscale clients use the tailnet DNS configuration by default. Managed devices can enforce `UseTailscaleDNSSettings=always` on Linux, macOS, and iOS. [Client preferences][preferences], [system policy][system-policy], [Apple shortcuts][shortcuts], [Linux DNS][linux-dns].

Use system resolution when testing on macOS. Commands such as `host` and `nslookup` can bypass the system resolver and fail even when MagicDNS works. `ping hermesagent`, a browser request, or `tailscale dns query hermesagent` gives a useful test. [MagicDNS][magicdns], [Tailscale CLI][tailscale-cli].

## HTTPS choices

### Tailscale Serve

Serve is the smallest default for one HTTP service. It provisions and renews the public certificate, terminates TLS, and keeps the service private to the tailnet. It also removes spoofed Tailscale identity headers before adding verified headers for the caller. GAH can keep its existing token and paired-device authentication. [Tailscale Serve][serve].

Enabling Tailscale HTTPS publishes the machine name and tailnet DNS name in Certificate Transparency logs. Access to the service remains restricted by the tailnet. Operators must rename sensitive machine names before enabling HTTPS. [Tailscale HTTPS][https].

### Caddy with a Tailscale certificate

This choice fits an installation that already needs Caddy routing. Caddy automatically recognizes `*.ts.net` site names and requests certificates from `tailscaled` during TLS handshakes. The integration renews certificates without a file-copy job. [Caddy automatic HTTPS][caddy-auto], [Caddy certificates on Tailscale][caddy-tailscale], [Tailscale HTTPS][https].

Do not combine Caddy and Serve for the same HTTPS listener. Select one TLS terminator. A second proxy adds another configuration and failure point without meeting a requirement in #943.

### Caddy internal CA

`tls internal` can issue a certificate for the bare `hermesagent` name. Every client must trust Caddy's root certificate. Caddy can install the root on its own host when it has permission, but remote clients and some browser trust stores need separate installation. [Caddy TLS directive][caddy-tls], [running Caddy][caddy-running].

Manual Apple enrollment has an extra step. After installing the certificate profile on iOS, the user must enable full trust under **Settings > General > About > Certificate Trust Settings**. Manually installed macOS roots can also require explicit TLS trust in Keychain Access. MDM or Apple Configurator can establish trust automatically. [Apple certificate trust][apple-trust], [Apple certificate management][apple-certs].

Installing an internal root gives that CA authority on each enrolled client. This is an inference from the certificate trust model, not a Caddy-specific warning. It adds root distribution, removal, rotation, and server-key protection to GAH operations.

Do not use `tls internal { on_demand }` for this fixed hostname. Caddy documents on-demand issuance for unknown names and warns that unrestricted public use can exhaust resources. A known `hermesagent` site does not need it. [Caddy TLS directive][caddy-tls], [Caddy automatic HTTPS][caddy-auto].

## Implementation checks

- Confirm that `hermesagent` resolves on Linux, macOS, and iOS.
- Confirm that `https://hermesagent.<tailnet-name>.ts.net` has a public trust chain.
- Confirm that direct off-tailnet access fails under the tailnet policy.
- Confirm that GAH still requires its bearer token or paired-device cookie through the selected proxy.
- Store the full HTTPS origin in `registry_central_url`. Keep `GAH_SERVER_HOST` on loopback when Serve fronts the process.

[magicdns]: https://tailscale.com/docs/features/magicdns
[machine-names]: https://tailscale.com/kb/1098/machine-names
[preferences]: https://tailscale.com/docs/features/client/manage-preferences
[system-policy]: https://tailscale.com/docs/features/tailscale-system-policies
[shortcuts]: https://tailscale.com/docs/features/mac-ios-shortcuts
[linux-dns]: https://tailscale.com/docs/reference/faq/dns-resolv-conf
[tailscale-cli]: https://tailscale.com/docs/reference/tailscale-cli
[https]: https://tailscale.com/docs/how-to/set-up-https-certificates
[serve]: https://tailscale.com/docs/features/tailscale-serve
[serve-cli]: https://tailscale.com/docs/reference/tailscale-cli/serve
[caddy-tailscale]: https://tailscale.com/docs/integrations/web-servers/caddy/caddy-certificates
[caddy-auto]: https://caddyserver.com/docs/automatic-https
[caddy-tls]: https://caddyserver.com/docs/caddyfile/directives/tls
[caddy-running]: https://caddyserver.com/docs/running
[apple-trust]: https://support.apple.com/en-us/102390
[apple-certs]: https://support.apple.com/en-euro/guide/deployment/depb5eff8914/web
