"""Exercise credential and notarization failures without accessing Apple or secrets."""

import base64
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parent
INPUTS = {
    "SIGNING_CERTIFICATE_BASE64": "fixture-certificate",
    "MACOS_CERTIFICATE_PASSWORD": "fixture-password",
    "MACOS_SIGNING_IDENTITY": "Developer ID Application: Fixture (TEAM)",
    "NOTARY_KEY_P8_BASE64": base64.b64encode(b"fixture-key").decode(),
    "NOTARY_KEY_ID": "FIXTURE",
    "NOTARY_ISSUER_ID": "fixture-issuer",
}


class NotarizationTests(unittest.TestCase):
    def test_workflow_checks_the_same_certificate_it_imports(self):
        workflow = (SCRIPTS.parent / "workflows/macos-e2e.yml").read_text()
        imported = workflow.split("name: Import the certificate")[1].split("run:")[0]
        secrets = re.findall(r"secrets\.([A-Z0-9_]+)", imported)
        certificate = next(name for name in secrets if name != "MACOS_CERTIFICATE_PASSWORD")
        self.assertIn("SIGNING_CERTIFICATE_BASE64: ${{ secrets." + certificate + " }}", workflow)

    def gate(self, values):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "output"
            result = subprocess.run(
                ["bash", str(SCRIPTS / "check-signing-inputs.sh")],
                env={"PATH": os.environ["PATH"], "GITHUB_OUTPUT": str(output), **values},
                capture_output=True, text=True, check=False,
            )
            return result, output.read_text() if output.exists() else ""

    def test_complete_and_absent_inputs(self):
        result, output = self.gate(INPUTS)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(output, "present=true\n")
        result, output = self.gate({})
        self.assertEqual(result.returncode, 0)
        self.assertEqual(output, "present=false\n")

    def test_every_missing_input_fails_without_exposing_values(self):
        for missing in INPUTS:
            with self.subTest(missing=missing):
                result, output = self.gate({k: v for k, v in INPUTS.items() if k != missing})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(missing, result.stdout)
                self.assertNotIn("present=true", output)
                for value in INPUTS.values():
                    self.assertNotIn(value, result.stdout + result.stderr)

    def test_required_run_cannot_skip_and_ad_hoc_cannot_pass(self):
        self.assertNotEqual(self.gate({"REQUIRE_NOTARIZATION": "true"})[0].returncode, 0)
        self.assertNotEqual(self.gate({**INPUTS, "MACOS_SIGNING_IDENTITY": "-"})[0].returncode, 0)

    def exercise_notary(self, status):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "bin"
            binary.mkdir()
            work = root / "work"
            work.mkdir()
            (work / "ReferenceApp.zip").write_bytes(b"before stapling")
            commands = {
                "openssl": "exit 0\n",
                "codesign": 'echo verify >> "$TRACE"\n',
                "xcrun": '''if [[ "$1" == notarytool ]]; then
  [[ "$3" == "$WORK/ReferenceApp.zip" && "$4" == --key && -f "$5" ]]
  [[ "$6" == --key-id && "$8" == --issuer ]]
  echo submit >> "$TRACE"
  printf '{"status":"%s"}\\n' "$STATUS"
else
  echo "$1 $2" >> "$TRACE"
fi
''',
                "spctl": 'echo assess >> "$TRACE"\n',
                "ditto": 'echo archive >> "$TRACE"\nprintf stapled > "$WORK/ReferenceApp.zip"\n',
            }
            for name, body in commands.items():
                path = binary / name
                path.write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + body)
                path.chmod(0o755)
            trace = root / "trace"
            result = subprocess.run(
                ["bash", str(SCRIPTS / "notarize-api-key.sh")],
                env={
                    **INPUTS, "PATH": str(binary) + os.pathsep + os.environ["PATH"],
                    "RUNNER_TEMP": str(root), "WORK": str(work),
                    "STATUS": status, "TRACE": str(trace),
                },
                capture_output=True, text=True, check=False,
            )
            self.assertEqual(list(root.glob("notary.*")), [])
            return result, trace.read_text().splitlines(), (work / "ReferenceApp.zip").read_bytes()

    def test_accepted_result_staples_validates_and_rearchives(self):
        result, trace, archive = self.exercise_notary("Accepted")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(trace, ["verify", "submit", "stapler staple", "stapler validate", "assess", "archive"])
        self.assertEqual(archive, b"stapled")

    def test_rejected_result_never_staples_or_distributes(self):
        result, trace, archive = self.exercise_notary("Invalid")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(trace, ["verify", "submit"])
        self.assertEqual(archive, b"before stapling")


if __name__ == "__main__":
    unittest.main()
