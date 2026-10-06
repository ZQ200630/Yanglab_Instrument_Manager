"""Identity probes exercise real drivers with fake transports, never instruments."""
import unittest
from unittest.mock import Mock
from Code.Utils.common import DriverError, DriverState
from Code.Utils.osa import AQ6370
from Code.Utils.pm400 import PM400
from Code.Utils.mdt693b import MDT693B, Axis
from Code.Debugs import test_pm400 as pm_helpers
from Code.Debugs import test_mdt693b as mdt_helpers
from App.worker.catalog import load_catalog

class IdentityProbeTests(unittest.TestCase):
    def osa(self):
        resource,manager,instrument=Mock(),Mock(),Mock()
        resource.query.return_value="YOKOGAWA,AQ6370D,SN,FW"
        manager.open_resource.return_value=resource
        create=Mock(return_value=instrument)
        driver=AQ6370(resource_manager_factory=lambda:manager,instrument_factory=create)
        self.addCleanup(lambda:(setattr(resource.close,"side_effect",None),driver.close()))
        return driver,resource,manager,instrument,create

    def test_osa_probe_does_not_abort_front_panel_scan(self):
        driver,resource,manager,instrument,create=self.osa()
        self.assertTrue(hasattr(driver,"probe_identity"),"Public identity probe missing")
        report=driver.probe_identity()
        self.assertEqual([call.args[0] for call in resource.query.call_args_list],["*IDN?"])
        create.assert_not_called()
        instrument.abort.assert_not_called()
        self.assertEqual(report.identity["model"],"AQ6370D")
        self.assertTrue(report.release_confirmed)
        self.assertEqual(driver.state,DriverState.DISCONNECTED)
        with self.assertRaises(TypeError): report.identity["model"]="other"

    def test_probe_close_failure_retains_owner(self):
        driver,resource,manager,instrument,create=self.osa()
        self.assertTrue(hasattr(driver,"probe_identity"),"Public identity probe missing")
        resource.close.side_effect=OSError("held resource")
        with self.assertRaises(DriverError) as caught: driver.probe_identity()
        self.assertIs(caught.exception.driver,driver)
        self.assertFalse(caught.exception.probe_report.release_confirmed)
        self.assertTrue(driver.has_resource_responsibility)
        create.assert_not_called()
        instrument.abort.assert_not_called()
        resource.close.side_effect=None
        driver.close()
        self.assertFalse(driver.has_resource_responsibility)
        self.assertFalse(caught.exception.probe_report.release_confirmed)

    def test_query_failure_with_successful_cleanup_preserves_evidence(self):
        driver,resource,manager,instrument,create=self.osa()
        self.assertTrue(hasattr(driver,"probe_identity"),"Public identity probe missing")
        resource.query.side_effect=OSError("query failed")
        with self.assertRaises(DriverError) as caught: driver.probe_identity()
        self.assertTrue(caught.exception.probe_report.release_confirmed)
        self.assertFalse(driver.has_resource_responsibility)
        create.assert_not_called()

    def test_pm400_probe_reads_only_identity_and_sensor(self):
        resource=pm_helpers.FakeVisaResource({"*IDN?":"THORLABS,PM400,SN,FW",
            "SYSTem:SENSor:IDN?":'"S130C","SN1","2025-01",1,2,371'})
        driver=PM400("USB0::0x1313::0x8078::1::INSTR",
                     resource_manager_factory=lambda:pm_helpers.FakeResourceManager(resource))
        self.addCleanup(driver.close)
        self.assertTrue(hasattr(driver,"probe_identity"),"Public identity probe missing")
        report=driver.probe_identity()
        self.assertEqual(resource.transactions,["*IDN?","SYSTem:SENSor:IDN?"])
        self.assertEqual(resource.mutating_commands,[])
        self.assertEqual(report.identity["serial"],"SN")
        self.assertEqual(report.observations["sensor"]["name"],"S130C")
        self.assertTrue(report.release_confirmed)

    def test_mdt_probe_preserves_nonzero_voltage_and_disarms_motion(self):
        device=mdt_helpers.ScriptedMDTDevice(x=20,y=30,z=40)
        factory=mdt_helpers.RecordingSerialFactory(device)
        driver=MDT693B("COM_TEST",serial_factory=factory)
        self.addCleanup(driver.close)
        self.assertTrue(hasattr(driver,"probe_identity"),"Public identity probe missing")
        report=driver.probe_identity()
        self.assertTrue(report.release_confirmed)
        self.assertEqual(report.observations["voltage_v"],{"X":20.0,"Y":30.0,"Z":40.0})
        self.assertFalse(report.observations["axis_command_known"])
        self.assertTrue(all(command.endswith("?") for command in device.commands))
        self.assertEqual(report.identity["model"],"MDT693B")

    def test_serial_profile_discloses_open_reset_risk(self):
        profile=load_catalog().profile("mdt693b","serial")
        self.assertEqual(profile.probe_mode,"readonly")
        self.assertFalse(profile.automatic_probe)
        self.assertTrue(any("DTR" in effect or "reset" in effect for effect in profile.open_effects))
        for model,profile_id in (("voltage","ch340-serial"),("gain","cp210x-serial")):
            self.assertEqual(load_catalog().profile(model,profile_id).probe_mode,"supervised")

    def test_probe_rejects_existing_measurement_session_without_abort(self):
        driver,resource,manager,instrument,create=self.osa()
        driver.connect()
        self.assertTrue(hasattr(driver,"probe_identity"),"Public identity probe missing")
        with self.assertRaises(DriverError): driver.probe_identity()
        instrument.abort.assert_not_called()
        self.assertEqual(driver.state,DriverState.READY)

