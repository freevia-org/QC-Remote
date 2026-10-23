#!/usr/bin/env python3
"""Build Store helper bundles from the matching Store-feature broker executable.

Does not install, register login items, or issue signing credentials. Final
role-specific signing is performed after assembly by package-macos-appstore.py.
"""
import argparse
import json
from pathlib import Path
import plistlib
import subprocess


def run(*args):
    subprocess.run(args, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--broker', type=Path, required=True)
    parser.add_argument('--arch', choices=['arm64', 'x86_64'], required=True)
    args = parser.parse_args()
    if not args.broker.is_file() or args.broker.is_symlink():
        parser.error('broker must be the executable built with macos-app-store,desktop-broker')
    architectures = subprocess.check_output(['lipo', '-archs', str(args.broker)], text=True).split()
    if architectures != [args.arch]:
        parser.error('broker architecture does not match the requested helper architecture')
    root = Path(__file__).resolve().parent.parent
    output = root / '.artifacts/builds/store'
    output.mkdir(parents=True, exist_ok=True)
    menu = output / 'QC Device Broker.app'
    login = output / 'QC Remote Login.app'
    for app in (menu, login):
        (app / 'Contents/MacOS').mkdir(parents=True, exist_ok=True)
    info = plistlib.loads((root / 'apps/windows/macos-broker/Info.plist').read_bytes())
    marker = ('qc-remote-resident-host-build:v1:' + info['CFBundleShortVersionString'] + ':end:macos-app-store').encode()
    if marker not in args.broker.read_bytes():
        parser.error('broker is stale or was not built with macos-app-store')
    config = json.loads((root / 'apps/windows/src-tauri/tauri.appstore.conf.json').read_text())
    info['CFBundleVersion'] = config['bundle']['macOS']['bundleVersion']
    (menu / 'Contents/Info.plist').write_bytes(plistlib.dumps(info))
    resources = menu / 'Contents/Resources/BrokerLED'
    resources.mkdir(parents=True, exist_ok=True)
    exporter = output / 'export-broker-icons'
    run(str(Path.home() / '.cargo/bin/rustc'), '--edition', '2021', str(root / 'apps/windows/macos-broker/export-icons.rs'), '-o', str(exporter))
    run(str(exporter), str(resources))
    run('ditto', str(args.broker), str(menu / 'Contents/MacOS/qc-device-broker'))
    login_info = dict(info, CFBundleIdentifier='org.freevia.qcremote.login', CFBundleExecutable='QCRemoteLogin', CFBundleName='QC Remote Login')
    (login / 'Contents/Info.plist').write_bytes(plistlib.dumps(login_info))
    for source, destination in [('main.swift', menu / 'Contents/MacOS/QCDeviceBroker'), ('login.swift', login / 'Contents/MacOS/QCRemoteLogin')]:
        run('xcrun', 'swiftc', '-O', '-swift-version', '5', '-target', args.arch + '-apple-macos13.0', '-framework', 'AppKit', str(root / 'apps/windows/macos-broker' / source), '-o', str(destination))
    for executable in (menu / 'Contents/MacOS/qc-device-broker',):
        run('codesign', '--force', '--sign', '-', str(executable))
    for app in (menu, login):
        run('codesign', '--force', '--sign', '-', str(app))


if __name__ == '__main__':
    main()
