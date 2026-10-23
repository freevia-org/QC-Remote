#!/usr/bin/env python3
"""Package existing CI-verified apps for Developer ID direct distribution.

No publishing, credential creation, or changes to installed apps. Signing keys
remain in the system keychain. The application itself never uses Keychain.
"""
import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import subprocess
import tempfile


def run(*args, capture=False):
    return subprocess.run(args, check=True, text=True, capture_output=capture)


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app', type=Path, required=True)
    parser.add_argument('--broker', type=Path, required=True)
    parser.add_argument('--identity', required=True, help='Installed Developer ID Application identity')
    parser.add_argument('--notary-profile', help='Existing notarytool profile; omit only to prepare an UNNOTARIZED candidate')
    parser.add_argument('--output', type=Path, required=True, help='New output directory (must not exist)')
    parser.add_argument('--source-commit', required=True)
    args = parser.parse_args()
    if not args.identity.startswith('Developer ID Application:'):
        parser.error('Direct downloads require Developer ID Application, not Apple Distribution or ad-hoc signing')
    identities = run('security', 'find-identity', '-v', '-p', 'codesigning', capture=True).stdout
    if f'"{args.identity}"' not in identities:
        parser.error('The requested Developer ID Application identity/private key is not installed')
    if len(args.source_commit) != 40 or any(c not in '0123456789abcdef' for c in args.source_commit):
        parser.error('source-commit must be a full Git commit SHA')
    versions = []
    for app in (args.app, args.broker):
        if app.is_symlink() or not app.is_dir():
            parser.error('Both app inputs must be actual application directories')
        with (app / 'Contents/Info.plist').open('rb') as handle:
            versions.append(plistlib.load(handle)['CFBundleShortVersionString'])
    if versions[0] != versions[1]:
        parser.error('Editor and broker versions do not match')
    # The call creates its own new directory; never overwrite another release.
    args.output.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryDirectory(prefix='qc-sign-', dir=args.output) as temporary:
        stage = Path(temporary)
        for source in (args.app, args.broker):
            destination = stage / source.name
            run('ditto', str(source), str(destination))
            # Sign inside-out. Do not use --deep to sign arbitrary nested code.
            paths = sorted(destination.rglob('*'), key=lambda p: len(p.parts), reverse=True)
            bundles = [p for p in paths if p.is_dir() and not p.is_symlink() and p.suffix in ('.app', '.framework', '.xpc', '.appex', '.bundle')]
            main_executables = set()
            for bundle in [destination] + bundles:
                info_path = bundle / 'Contents/Info.plist'
                if info_path.exists():
                    with info_path.open('rb') as handle:
                        executable = plistlib.load(handle).get('CFBundleExecutable')
                    if executable:
                        main_executables.add(bundle / 'Contents/MacOS' / executable)
            for path in paths:
                if path.is_symlink():
                    continue
                is_code = False
                if path.is_file():
                    with path.open('rb') as handle:
                        is_code = handle.read(4) in (b'\xcf\xfa\xed\xfe', b'\xfe\xed\xfa\xcf', b'\xca\xfe\xba\xbe', b'\xbe\xba\xfe\xca', b'\xca\xfe\xba\xbf')
                if is_code and path not in main_executables:
                    run('codesign', '--force', '--sign', args.identity, '--options', 'runtime', '--timestamp', str(path))
            for bundle in bundles:
                run('codesign', '--force', '--sign', args.identity, '--options', 'runtime', '--timestamp', str(bundle))
            run('codesign', '--force', '--sign', args.identity, '--options', 'runtime', '--timestamp', str(destination))
            run('codesign', '--verify', '--deep', '--strict', str(destination))
        (stage / 'Applications').symlink_to('/Applications')
        architectures = run('lipo', '-archs', str(stage / args.app.name / 'Contents/MacOS/qc-remote'), capture=True).stdout.strip().replace(' ', '-')
        suffix = '' if args.notary_profile else '-signed-unnotarized'
        broker_architectures = run('lipo', '-archs', str(stage / args.broker.name / 'Contents/MacOS/qc-device-broker'), capture=True).stdout.strip().replace(' ', '-')
        if set(architectures.split('-')) != set(broker_architectures.split('-')):
            raise SystemExit('App and broker architectures do not match')
        image = args.output / f'QC-Remote-macOS-{versions[0]}-{architectures}{suffix}.dmg'
        run('hdiutil', 'create', '-volname', f'QC Remote {versions[0]}', '-srcfolder', str(stage), '-format', 'UDZO', str(image))
        run('codesign', '--sign', args.identity, '--timestamp', str(image))
        if not args.notary_profile:
            digest = sha256(image)
            manifest = {'sourceCommit': args.source_commit, 'version': versions[0], 'architectures': architectures, 'asset': image.name, 'sha256': digest, 'notarized': False, 'published': False}
            (args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
            print(json.dumps(manifest, indent=2))
            return
        result = run('xcrun', 'notarytool', 'submit', str(image), '--keychain-profile', args.notary_profile, '--wait', '--output-format', 'json', capture=True)
        receipt = json.loads(result.stdout)
        (args.output / 'notarization.json').write_text(json.dumps(receipt, indent=2) + '\n')
        if receipt.get('status') != 'Accepted':
            raise SystemExit('Apple did not accept notarization; nothing is ready to publish')
        run('xcrun', 'stapler', 'staple', str(image))
        run('xcrun', 'stapler', 'validate', str(image))
        run('hdiutil', 'verify', str(image))
        run('spctl', '--assess', '--type', 'open', '--context', 'context:primary-signature', str(image))
        digest = sha256(image)
        (args.output / 'SHA256SUMS').write_text(f'{digest}  {image.name}\n')
        manifest = {'sourceCommit': args.source_commit, 'version': versions[0], 'architectures': architectures, 'asset': image.name, 'sha256': digest, 'notarizationId': receipt.get('id'), 'notarized': True, 'published': False}
        (args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        print(json.dumps(manifest, indent=2))


if __name__ == '__main__':
    main()
