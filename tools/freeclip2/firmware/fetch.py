#!/usr/bin/env python3
"""Ask Huawei's OTA server for the newest FreeClip 2 firmware and download it (FINDINGS "Firmware image").

usage: python3 -I tools/freeclip2/firmware/fetch.py SERIAL CURRENT_VERSION OUT_DIR
SERIAL is the earbuds' serial number (MBB 1/7 TLV 9) and CURRENT_VERSION their firmware string (MBB 1/7 TLV 7),
e.g. "HarmonyOS 7.0.0.526(F003H003C00)". This sends the serial number to Huawei: ask the user first.
Writes OUT_DIR/check.json, filelist.xml and update_accessory_fw.zip; checks the zip's size, MD5 and SHA-256.
"""
import hashlib, json, os, sys, urllib.request
import xml.etree.ElementTree as ET

ZIP = "update_accessory_fw.zip"
CHECK = "https://update-dre.platform.hicloud.com/smartAccessory/v2/Check.action?latest=true&defenceHijack=true"


def fetch(url, data=None):
    req = urllib.request.Request(url, data=data, headers={"Content-Type": "application/json"} if data else {})
    with urllib.request.urlopen(req, timeout=60) as r:
        return r.read()


def main(serial, version, out):
    os.makedirs(out, exist_ok=True)
    body = {"rules": {"deviceName": "HuaweiBTFT0027", "packageName": "com.huawei.BTFT0027", "deviceId": serial,
                      "packageType": "full", "packageVersionCode": version, "firmware": version, "os": "audioapp"}}
    check = json.loads(fetch(CHECK, json.dumps(body).encode()))
    open(f"{out}/check.json", "w").write(json.dumps(check, indent=1))
    assert check.get("status") == "0", f"check failed: {check}"
    components = check.get("components") or []
    if not components:
        print(f"no newer firmware than {version}")
        return
    comp = components[0]  # the app also takes the first component (FINDINGS "Firmware image")
    print(f"newest: {comp['version']} (created {comp['createTime']})")
    base = comp["url"]
    filelist = fetch(base + "full/filelist.xml")
    open(f"{out}/filelist.xml", "wb").write(filelist)
    entries = [f for f in ET.fromstring(filelist).iter("file") if f.findtext("spath") == ZIP]
    assert len(entries) == 1, f"filelist.xml lists {ZIP} {len(entries)} times (expected once); see {out}/filelist.xml"
    f = entries[0]
    data = fetch(base + "full/" + ZIP)
    assert len(data) == int(f.findtext("size")), f"{ZIP}: size mismatch"
    assert hashlib.md5(data).hexdigest().lower() == f.findtext("md5").lower(), f"{ZIP}: MD5 mismatch"
    assert hashlib.sha256(data).hexdigest().lower() == f.findtext("sha256").lower(), f"{ZIP}: SHA-256 mismatch"
    open(f"{out}/{ZIP}", "wb").write(data)
    print(f"downloaded {ZIP} ({len(data)} bytes, hashes ok)")


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    main(*sys.argv[1:])
