"""Hardware GPU daily rows are required only on hardware Vulkan runners."""
import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from tools.ci import daily_summary, ignored_shards, ignored_tests


ADAPTER = ("vulkan adapter[{index}]: name={name} type={kind} api=1.3.0 "
           "max_storage_buffer_binding_size=134217728 (128 MiB) "
           "max_storage_buffers_per_shader_stage=8 max_texture_dimension_2d=16384")
SOFTWARE = ADAPTER.format(index=0, name="llvmpipe (not type=discrete-gpu)", kind="cpu")
UNAVAILABLE = "vulkan adapter: unavailable (no-physical-devices)"


class HardwareGpuDailyTests(unittest.TestCase):
    def test_probe_has_hardware_if_any_adapter_is_not_a_cpu(self):
        self.assertFalse(ignored_tests.has_hardware_vulkan([]))
        self.assertFalse(ignored_tests.has_hardware_vulkan([SOFTWARE]))
        self.assertFalse(ignored_tests.has_hardware_vulkan([UNAVAILABLE]))
        for kind in ("discrete-gpu", "integrated-gpu", "virtual-gpu", "other"):
            with self.subTest(kind=kind):
                hardware = ADAPTER.format(index=1, name="GPU", kind=kind)
                self.assertTrue(ignored_tests.has_hardware_vulkan([SOFTWARE, hardware]))

    def test_hardware_rows_are_skipped_or_required_in_both_daily_routes(self):
        hardware = ADAPTER.format(index=1, name="GPU", kind="discrete-gpu")
        # Use the actual hardware policy for the package shards, plus a row owned by daily-rows.
        rows = [row for row in ignored_tests.daily_rows() if row.reason == "hardware-gpu"]
        rows.append(ignored_tests.Row("sunrise-core", "gpu::hardware", "lib", "daily",
                                      "daily-gpu.txt", "hardware-gpu"))
        ordinary = ignored_tests.Row("sunrise-client-render", "gfx::ordinary", "lib", "daily", "daily-gpu.txt", "gpu")
        expected = [{"package": row.package, "test": row.test, "reason": "hardware-gpu"} for row in rows]
        packages = sorted({row.package for row in rows if row.package != "sunrise-core"})
        shards = [*ignored_shards.expand(packages), "daily-rows"]
        for detected, summaries in ((False, [SOFTWARE]), (False, [UNAVAILABLE]), (True, [SOFTWARE, hardware])):
            with (self.subTest(summaries=summaries), tempfile.TemporaryDirectory() as scratch,
                  patch.object(ignored_tests, "listed_entries", return_value=[*rows, ordinary]),
                  patch.object(ignored_tests, "has_vulkan", return_value=True),
                  patch.object(ignored_tests, "vulkan_adapter_summaries", return_value=summaries),
                  patch.object(ignored_tests, "built_test_binaries", return_value={}),
                  patch.object(ignored_tests, "run", side_effect=lambda package, *_args, **_kwargs:
                               {ordinary.test: True} if package == ordinary.package else {}) as run,
                  contextlib.redirect_stdout(io.StringIO()) as log):
                path = Path(scratch) / "report.json"
                # The fake binaries report no result for any hardware row: red only if they were required.
                self.assertEqual(ignored_tests.main(shards, path), int(detected))
                verdict = json.loads(path.read_text())
                self.assertEqual(verdict["passed"], [{"package": ordinary.package, "test": ordinary.test}])
                self.assertFalse(verdict["incomplete"])
                selected = {(run_call.args[0], name) for run_call in run.call_args_list
                            for name in run_call.kwargs.get("only", ([], []))[1]}
                excluded = {(run_call.args[0], name) for run_call in run.call_args_list for name in run_call.args[1]}
                if detected:
                    self.assertEqual(selected, {("sunrise-core", "gpu::hardware")})
                    self.assertFalse(excluded)
                    self.assertEqual(verdict["skipped"], [])
                    self.assertCountEqual(
                        [(item["package"], item["test"], item["kind"]) for item in verdict["problems"]],
                        [(row.package, row.test, "no-longer-runs") for row in rows],
                    )
                else:
                    self.assertFalse(selected)
                    self.assertEqual(excluded, {(row.package, row.test) for row in rows if row.package in packages})
                    self.assertEqual(verdict["problems"], [])
                    self.assertCountEqual(verdict["skipped"], expected)
                    for row in rows:
                        self.assertEqual(log.getvalue().count(f"{row.package} {row.test}: SKIPPED (hardware-gpu:"), 1)

    def test_daily_summary_shows_hardware_skips_without_closing_their_issues(self):
        package, test = "sunrise-client-render", "gfx::hardware"
        job = f"shards / ignored-tests-shards ({package})"
        title = f"Daily: {package} {test}"
        report = {"shards": [package], "passed": [], "problems": [], "incomplete": [],
                  "skipped": [{"package": package, "test": test, "reason": "hardware-gpu"}]}
        with tempfile.TemporaryDirectory() as scratch:
            folder = Path(scratch)
            artifact = folder / "daily-report-shard-0"
            artifact.mkdir()
            (artifact / "daily-report.json").write_text(json.dumps(report))
            summary = folder / "summary.md"
            env = {"COMMIT": "a" * 40, "RESULTS": '{"shards":{"result":"success"}}', "DRY_RUN": "true",
                   "GITHUB_REPOSITORY": "owner/repo", "GITHUB_RUN_ID": "1", "GITHUB_RUN_ATTEMPT": "1",
                   "REPORTS": scratch, "GITHUB_STEP_SUMMARY": str(summary)}
            with (patch.dict(os.environ, env, clear=True),
                  patch.object(daily_summary, "expected_shards", return_value=[job]),
                  patch.object(daily_summary, "pages", side_effect=[
                      [{"name": job, "conclusion": "success"}], [{"title": title, "number": 10}],
                  ]),
                  contextlib.redirect_stdout(io.StringIO()) as log):
                self.assertEqual(daily_summary.main(), 0)
            text = summary.read_text()
            self.assertIn(f"- `{package} {test}`: SKIPPED (`hardware-gpu`)", text)
            self.assertIn(text, log.getvalue())
            self.assertNotIn("Close #10", text)


if __name__ == "__main__":
    unittest.main()
