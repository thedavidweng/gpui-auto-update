# Key management

Every update artifact is signed with one Ed25519 private key, and every
installed copy of the application trusts the matching public key. The same
key pair signs Sparkle appcast artifacts on macOS and the native feed
artifacts on Windows and Linux (see [feed-format.md](feed-format.md)).

- The **public key** ships inside the application: `SUPublicEDKey` in
  `Info.plist` on macOS, and the trust key the application passes to the
  updater on Windows and Linux. It is not a secret.
- The **private key** never ships in the application, never goes into source
  control, and is never passed on a command line.

The `gpui-auto-update keys` command creates, imports, inspects, and checks
keys. It uses the same formats as Sparkle's `generate_keys`, so keys move
freely between this tool, `generate_keys`, `sign_update`, and
`generate_appcast`.

## Key formats

| Item | Encoding |
| --- | --- |
| Public key | Standard base64 of the 32-byte Ed25519 public key (44 characters). Identical to `SUPublicEDKey`. |
| Private key | Standard base64 of either the 32-byte Ed25519 seed (what Sparkle and this tool generate) or Sparkle's legacy 96-byte format (64-byte expanded secret followed by the public key). |
| Signature | Standard base64 of the 64-byte Ed25519 signature over the raw artifact bytes. Identical to `sparkle:edSignature`. |

A private key file holds only the base64 text, as written by
`generate_keys -x`. Surrounding whitespace is ignored on input. Legacy 96-byte
keys keep working and sign identically, but their seed cannot be recovered, so
they are stored and exported in their 96-byte form.

## Creating a key

On a release engineer's Mac, the recommended home for the key is Sparkle's
login Keychain item, which `sign_update` and `generate_appcast` can use
without the key ever touching disk:

```sh
gpui-auto-update keys generate --keychain --sparkle-bin /path/to/Sparkle/bin
```

This runs Sparkle's own `generate_keys`. If the Keychain account already holds
a key, that key is kept. Use `--account <name>` to keep one key per
organization or product (Sparkle's default account is `ed25519`).
`--sparkle-bin` may be replaced by the `SPARKLE_BIN` environment variable or
by having `generate_keys` on `PATH`.

Anywhere else, or to create the key that CI will use, write it to a new file:

```sh
gpui-auto-update keys generate --output signing-key.txt
```

The file is created with owner-only permissions on Unix (on Windows, it
inherits the directory's permissions, so use a private directory), and an
existing file is never overwritten. Only the public key is printed; the
private key is never written to the terminal. Copy the file into your secret
store and offline backup, then delete it.

Both forms print the public key on standard output. Put it in the
application's configuration. **Choose the key before your first public
release:** once copies are installed, changing the public key requires the
[rotation procedure](#rotating-the-signing-key).

## Importing an existing key

A key exported by `generate_keys -x` (or used with `sign_update
--ed-key-file`) can be checked and stored elsewhere:

```sh
# Into Sparkle's Keychain item (macOS)
gpui-auto-update keys import --key-file exported-key.txt --keychain

# Into a new owner-only file
gpui-auto-update keys import --key-stdin --output signing-key.txt < exported-key.txt
```

`import` validates the key before storing it. When importing into the
Keychain, the key is handed to `generate_keys -f` through a temporary file in
a fresh owner-only directory, which is overwritten and removed immediately
afterwards, and the command then confirms that the Keychain account really
holds the imported key. If the account already held a different key, the
command fails rather than reporting success.

## Supplying the private key

Commands that need a private key accept it only through one of these
sources:

| Option | Source |
| --- | --- |
| `--key-stdin` | Standard input. |
| `--key-env <VAR>` | The named environment variable, for example `SPARKLE_PRIVATE_KEY`. |
| `--key-file <path>` | A file containing the base64 key. |
| `--keychain [--account <name>]` | Sparkle's login Keychain item (macOS). Only the public key is read. |

There is deliberately no option that takes the key itself as an argument:
arguments are visible to other processes, stored in shell history, and often
echoed into CI logs. Options such as `--private-key` or `--key=...` are
rejected, and unrecognized arguments are never echoed back in error messages
in case they are a pasted key. No command prints a private key, and error
messages name the source that failed (for example "environment variable
`SPARKLE_PRIVATE_KEY`") without quoting its contents.

## Checking a key pair

Before signing anything, confirm that the private key belongs to the public
key the installed applications trust:

```sh
gpui-auto-update keys check --public-key "$APP_PUBLIC_ED_KEY" --key-env SPARKLE_PRIVATE_KEY
```

If the keys differ, the command exits with status 1 and prints both public
keys. A release signed with the wrong key would be rejected by every
installed copy, so treat this as a hard failure in CI. Use
`gpui-auto-update keys public-key <source>` to print the public key of a
private key, for example to compare it with an `Info.plist`.

## CI setup

1. Store the base64 private key as a masked CI secret (for example a GitHub
   Actions secret named `SPARKLE_PRIVATE_KEY`).
2. Store the public key as an ordinary variable, or read it from the
   application's configuration in the repository.
3. Expose the secret only to the signing step, as an environment variable,
   and pass it on to tools through `--key-env` or standard input:

```yaml
- name: Check signing key
  env:
    SPARKLE_PRIVATE_KEY: ${{ secrets.SPARKLE_PRIVATE_KEY }}
  run: gpui-auto-update keys check --public-key "$APP_PUBLIC_ED_KEY" --key-env SPARKLE_PRIVATE_KEY

- name: Sign with Sparkle (macOS)
  env:
    SPARKLE_PRIVATE_KEY: ${{ secrets.SPARKLE_PRIVATE_KEY }}
  run: printf '%s' "$SPARKLE_PRIVATE_KEY" | "$SPARKLE_BIN/sign_update" --ed-key-file - MyApp.zip
```

Do not write the key to a file in the workspace, do not `echo` it without
piping, and do not enable shell tracing (`set -x`) in steps that handle it.
Pull requests from forks must not be able to read the secret.

## The insecure test key

For local development and tests, the project publishes one test key pair.
Its seed is the 32 ASCII bytes `gpui-auto-update-INSECURE-test!!`
(`INSECURE_TEST_KEY_SEED` in `gpui-auto-update-core`), and its public key is:

```text
X4kvjOoTZUsPKjjF0W/qzutLOb3d9LPmYZ4szZGg8wI=
```

Because the private key is public, anyone can sign with it. It exists so that
development builds can exercise real signature verification without access
to the production key, and it is marked so that it cannot pass as a
production key:

- `gpui-auto-update keys generate --test --output <path>` writes it, with a
  warning;
- `keys check` refuses it, even when the pair matches, unless
  `--allow-test-key` is given;
- `keys import` refuses to import it;
- `keys public-key` warns when it sees it;
- `TrustedKey::is_insecure_test_key` lets other tooling, such as doctor
  checks and release scripts, recognize it from the public key alone.

Never configure the test public key in a build you distribute: anyone could
then publish updates for it. Disposable keys created during automated tests
should be generated fresh and discarded, never reused as release keys.

## Rotating the signing key

Every installed copy verifies updates with the public key compiled into
*that* copy. **Never simply replace the public key in the next release.**
Installed copies would reject every update signed with the new key, and users
would be stranded on their current version with no automatic way forward.

Rotation therefore needs a *bridge release*: a release that is signed with the
old key (so existing installations accept it) but contains the new public key
(so it accepts future releases). The procedure below also moves to a new feed
URL, so that installations that have not yet taken the bridge release are
never offered a release they cannot verify, however many versions they skip.

1. **Create the new key pair** as described in
   [Creating a key](#creating-a-key), using a new Keychain account or a new
   CI secret name (for example `SPARKLE_PRIVATE_KEY_2027`). Keep the old key.
2. **Prepare the bridge release.** In this release:
   - set the trusted public key (`SUPublicEDKey` and the Windows and Linux
     trust key) to the **new** public key;
   - set the feed URLs (`SUFeedURL` and the native feed URLs) to **new**
     URLs, for example `.../v2/appcast.xml`;
   - on macOS, keep the same Apple code-signing identity. Sparkle accepts an
     update that changes either the EdDSA key or the code-signing identity,
     not both at once.
3. **Sign the bridge release with the old key**, and check the pair against
   the public key that the *currently installed* versions trust:

   ```sh
   gpui-auto-update keys check --public-key "$OLD_PUBLIC_KEY" --key-env SPARKLE_PRIVATE_KEY
   ```

   Publish it in the **old** feeds. From now on the old feeds only ever list
   the bridge release (and older releases); they are frozen.
4. **Start the new feeds** at the new URLs. List the bridge release there
   with a signature made by the **new** key (the same artifact can carry a
   different signature in each feed), and sign every later release with the
   new key only. Check the new pair against the new public key before each
   release.
5. **Keep the old feeds and the bridge artifacts online** for as long as old
   installations may exist (typically years). An installation that starts
   after a long time offline still finds the bridge in the old feed, verifies
   it with the old key, and moves to the new key and feed.
6. **Retire the old private key** only when you are confident no installation
   still needs the bridge: delete it from CI secrets and Keychains, and keep
   an offline, access-controlled copy if policy requires it.

Before publishing the bridge, test it end to end: install a build that trusts
the old key, update it to the bridge, and then update it to a release signed
only with the new key.

### If the private key is lost

A lost key cannot be rotated: no update signed with any other key will be
accepted by installed copies. Users have to download and install a new build
manually, so keep at least one offline backup of every key that installed
copies trust.

### If the private key is compromised

Rotate as above, as soon as possible, and announce the incident. The bridge
release is still signed with the compromised key, because that is the only
key installed copies accept; until users take the bridge, an attacker who
controls their network path or feed host can also serve them updates signed
with the stolen key. Feed and artifact hosting over HTTPS, and on macOS
Sparkle's additional check of the update's Apple code signature, limit what
the attacker can do with the key alone.
