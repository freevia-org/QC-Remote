#!/usr/bin/env python3
"""Assemble role-specific sandbox signatures for a Store-feature app bundle.

Development mode creates an executable sandbox test candidate. Submission mode
embeds the supplied production profile and signs a Mac App Store installer.
Neither mode installs, uploads, publishes, or changes the installed editor.
"""
import argparse
import datetime
import json
from pathlib import Path
import plistlib
import subprocess


def run(*args, capture=False):
    return subprocess.run(args, check=True, text=True, capture_output=capture)


def info(app):
    return plistlib.loads((app / 'Contents/Info.plist').read_bytes())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app', type=Path, required=True, help='Bundle built with macos-app-store,desktop-broker')
    parser.add_argument('--identity', required=True)
    parser.add_argument('--installer-identity')
    parser.add_argument('--profile', type=Path)
    parser.add_argument('--development', action='store_true')
    parser.add_argument('--output', type=Path, required=True, help='New directory')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    entitlements = root / 'apps/windows/src-tauri/macos'
    identities = run('security', 'find-identity', '-v', '-p', 'basic', capture=True).stdout
    if f'"{args.identity}"' not in identities:
        parser.error('The requested signing identity/private key is not installed')
    if args.development:
        if not args.identity.startswith(('Apple Development:', 'Developer ID Application:')):
            parser.error('Sandbox execution tests require a development or Developer ID identity')
    elif not args.identity.startswith(('Apple Distribution:', '3rd Party Mac Developer Application:')):
        parser.error('Store submission requires Apple Distribution, not Developer ID')
    if not args.development and (not args.profile or not args.installer_identity):
        parser.error('Store submission requires a production profile and installer identity')
    if args.app.is_symlink() or not args.app.is_dir():
        parser.error('app must be an actual application directory')
    source_info = info(args.app)
    if source_info.get('CFBundleIdentifier') != 'org.freevia.qcremote':
        parser.error('Unexpected app bundle identifier')
    marker = ('qc-remote-macos-app-store-build:v1:' + source_info['CFBundleShortVersionString'] + ':end').encode()
    if marker not in (args.app / 'Contents/MacOS' / source_info['CFBundleExecutable']).read_bytes():
        parser.error('Editor is stale or was not built with macos-app-store')
    profile = None
    if not args.development:
        profile = plistlib.loads(subprocess.check_output(['security', 'cms', '-D', '-i', str(args.profile)]))
        if profile.get('Entitlements', {}).get('com.apple.application-identifier') != '8C782Q826S.org.freevia.qcremote':
            parser.error('Profile does not authorize QC Remote for Freevia EOOD')
        if profile['ExpirationDate'].replace(tzinfo=datetime.timezone.utc) <= datetime.datetime.now(datetime.timezone.utc):
            parser.error('The provisioning profile has expired')
        if profile.get('ProvisionedDevices') or profile.get('Entitlements', {}).get('get-task-allow'):
            parser.error('Use a Mac App Store production provisioning profile')
        if f'"{args.installer_identity}"' not in identities or not args.installer_identity.startswith('3rd Party Mac Developer Installer:'):
            parser.error('The Mac Installer Distribution identity/private key is not installed')
    allowed = {args.app / 'Contents/Helpers/QC Device Broker.app', args.app / 'Contents/Library/LoginItems/QC Remote Login.app'}
    if set(args.app.rglob('*.app')) != allowed:
        parser.error('Unexpected or missing nested app; rebuild from a clean bundle output')
    args.output.mkdir(parents=True, exist_ok=False)
    app = args.output / 'QC Remote.app'
    run('ditto', str(args.app), str(app))
    menu = app / 'Contents/Helpers/QC Device Broker.app'
    login = app / 'Contents/Library/LoginItems/QC Remote Login.app'
    for bundle, identifier in [(menu, 'org.freevia.qcremote.broker-menu'), (login, 'org.freevia.qcremote.login')]:
        if info(bundle).get('CFBundleIdentifier') != identifier:
            raise SystemExit('Missing or incorrectly identified Store helper: ' + str(bundle))
    marker = ('qc-remote-resident-host-build:v1:' + source_info['CFBundleShortVersionString'] + ':end:macos-app-store').encode()
    for broker in [menu / 'Contents/MacOS/qc-device-broker', *[p for p in (app / 'Contents/MacOS').iterdir() if p.name.startswith('qc-device-broker')]]:
        if marker not in broker.read_bytes():
            raise SystemExit('Broker is stale or was not built with macos-app-store: ' + str(broker))
    if (app / 'Contents/embedded.provisionprofile').exists():
        raise SystemExit('Input must be a fresh assembled bundle without a profile')
    app_entitlements = plistlib.loads((entitlements / 'store-app.entitlements').read_bytes())
    menu_entitlements = dict(app_entitlements)
    menu_entitlements.pop('com.apple.security.files.user-selected.read-write')
    menu_entitlements.pop('com.apple.security.files.bookmarks.app-scope')
    if not args.development:
        run('ditto', str(args.profile), str(app / 'Contents/embedded.provisionprofile'))
        app_entitlements['com.apple.application-identifier'] = '8C782Q826S.org.freevia.qcremote'
        app_entitlements['com.apple.developer.team-identifier'] = '8C782Q826S'
    app_entitlement_path = args.output / 'app.entitlements'
    menu_entitlement_path = args.output / 'menu.entitlements'
    app_entitlement_path.write_bytes(plistlib.dumps(app_entitlements))
    menu_entitlement_path.write_bytes(plistlib.dumps(menu_entitlements))
    main_executables = {bundle / 'Contents/MacOS' / info(bundle)['CFBundleExecutable'] for bundle in (app, menu, login)}
    architecture = set(run('lipo', '-archs', str(app / 'Contents/MacOS' / source_info['CFBundleExecutable']), capture=True).stdout.split())
    code = []
    for path in app.rglob('*'):
        if path.is_symlink() or not path.is_file() or path in main_executables:
            continue
        with path.open('rb') as handle:
            magic = handle.read(4)
        if magic in (b'\xcf\xfa\xed\xfe', b'\xfe\xed\xfa\xcf', b'\xca\xfe\xba\xbe', b'\xbe\xba\xfe\xca', b'\xca\xfe\xba\xbf'):
            code.append(path)
    for path in [*code, *main_executables]:
        if set(run('lipo', '-archs', str(path), capture=True).stdout.split()) != architecture:
            raise SystemExit('Architecture mismatch: ' + str(path))
    def sign(path, entitlement):
        run('codesign', '--force', '--sign', args.identity, '--options', 'runtime', '--entitlements', str(entitlement), str(path))
    for path in code:
        sign(path, entitlements / 'store-child.entitlements')
    sign(menu, menu_entitlement_path)
    sign(login, entitlements / 'store-login.entitlements')
    sign(app, app_entitlement_path)
    run('codesign', '--verify', '--deep', '--strict', str(app))
    for bundle in (app, menu, login):
        signature = run('codesign', '--display', '--verbose=4', str(bundle), capture=True)
        if 'TeamIdentifier=8C782Q826S' not in signature.stderr:
            raise SystemExit('Every Store bundle must be signed by the Freevia EOOD team')
    if profile:
        certificate_prefix = args.output / 'signing-cert-'
        run('codesign', '--display', '--extract-certificates=' + str(certificate_prefix), str(app))
        if (args.output / 'signing-cert-0').read_bytes() not in profile['DeveloperCertificates']:
            raise SystemExit('The app signature certificate is not authorized by the supplied profile')
    package = None
    if not args.development:
        package = args.output / ('QC-Remote-' + source_info['CFBundleShortVersionString'] + '-AppStore.pkg')
        run('productbuild', '--component', str(app), '/Applications', '--sign', args.installer_identity, str(package))
        run('pkgutil', '--check-signature', str(package))
    manifest = {'version': source_info['CFBundleShortVersionString'], 'architecture': sorted(architecture), 'development': args.development, 'identity': args.identity, 'profileUUID': profile['UUID'] if profile else None, 'app': str(app.resolve()), 'package': str(package.resolve()) if package else None, 'sandboxE2EVerified': False, 'uploaded': False}
    (args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(json.dumps(manifest, indent=2))


if __name__ == '__main__':
    main()
