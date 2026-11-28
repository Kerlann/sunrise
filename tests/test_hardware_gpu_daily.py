"""Hardware GPU daily rows are required only on hardware Vulkan runners."""
import contextlib
import io
import tempfile
import unittest
from pathlib import Path
from unittest.mock import call, patch

from tools.ci import ignored_tests


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
        package = "sunrise-client-render"
        grouped = "sunrise-core"
        hardware = ADAPTER.format(index=1, name="GPU", kind="discrete-gpu")
        with tempfile.TemporaryDirectory() as scratch:
            policy = Path(scratch)
            (policy / "daily-gpu.txt").write_text(
                f"{package} gfx::ordinary lib daily gpu\n"
                f"{package} gfx::hardware lib daily hardware-gpu\n"
                f"{grouped} gpu::hardware lib daily hardware-gpu\n"
            )
            for detected, summaries in ((False, [SOFTWARE]), (False, [UNAVAILABLE]), (True, [SOFTWARE, hardware])):
                with (self.subTest(summaries=summaries),
                      patch.object(ignored_tests, "POLICY", policy),
                      patch.object(ignored_tests, "RETIRED", policy / "absent.txt"),
                      patch.object(ignored_tests, "has_vulkan", return_value=True),
                      patch.object(ignored_tests, "vulkan_adapter_summaries", return_value=summaries),
                      patch.object(ignored_tests, "built_test_binaries", return_value={}),
                      patch.object(ignored_tests, "run", side_effect=[{"gfx::ordinary": True}, {"gpu::hardware": True}]) as run,
                      contextlib.redirect_stdout(io.StringIO()) as log):
                    # The shard never reports gfx::hardware: that is only a failure if it was required.
                    self.assertEqual(ignored_tests.main([package, "daily-rows"]), int(detected))
                    if detected:
                        self.assertEqual(run.call_args_list, [
                            call(package, []),
                            call(grouped, [], only=(["lib"], ["gpu::hardware"]), binaries=None),
                        ])
                        self.assertIn(f"::error::{package} gfx::hardware:", log.getvalue())
                    else:
                        run.assert_called_once_with(package, ["gfx::hardware"])
                        self.assertIn("hardware-gpu daily rows are skipped", log.getvalue())


if __name__ == "__main__":
    unittest.main()
