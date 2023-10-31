#!/usr/bin/python3
import sys
import subprocess

SYSTEMD_UNITS_PATH = '/etc/systemd/system/'

def get_env() -> str:
    assert len(sys.argv) == 2
    assert sys.argv[1] in ('dev', 'prod')
    return sys.argv[1]

def install_template(in_path: str, out_path: str, subs: list[tuple[str, str]]):
    with open(in_path, 'r') as f:
        text = f.read()
    for key, value in subs:
        text = text.replace(f'{{{{{key}}}}}', value)
    with open(out_path, 'w') as f:
        f.write(text)

def install_unit_files(environment: str) -> list[str]:
    managrams_name = f'mirrorbot-managrams-{environment}'
    # managrams
    install_template(
        'managrams.service.tmpl', 
        f'{SYSTEMD_UNITS_PATH}{managrams_name}.service', 
        (('ENVIRONMENT', environment),)
    )
    install_template(
        'managrams.timer.tmpl', 
        f'{SYSTEMD_UNITS_PATH}{managrams_name}.timer', 
        ()
    )
    # sync
    sync_name = f'mirrorbot-sync-{environment}'
    install_template(
        'sync.service.tmpl', 
        f'{SYSTEMD_UNITS_PATH}{sync_name}.service', 
        (('ENVIRONMENT', environment), ('MANAGRAMS_SERVICE', f'{managrams_name}.service'))
    )
    install_template(
        'sync.timer.tmpl', 
        f'{SYSTEMD_UNITS_PATH}{sync_name}.timer', 
        ()
    )
    return [f'{managrams_name}.timer', f'{sync_name}.timer']

def enable_units(units: list[str]):
    subprocess.run(["sudo", "systemctl", "daemon-reload"]).check_returncode()
    for unit in units:
        subprocess.run(["sudo", "systemctl", "enable", unit]).check_returncode()
        subprocess.run(["sudo", "systemctl", "restart", unit]).check_returncode()

if __name__ == '__main__':
    environment = get_env()
    units = install_unit_files(environment)
    enable_units(units)
