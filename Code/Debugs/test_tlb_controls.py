"""Bounded command sequences; no hardware acquisition."""
import unittest
from Code.Debugs.test_tlb6700 import Script
from Code.Utils.tlb6700 import TLB6700
from Code.Utils.common import DriverState, InstrumentSafetyError, InstrumentProtocolError


class ScanScript(Script):
    def __init__(self, head='6722-P'):
        super().__init__(head=head)
        self.replies.update({'SOUR:WAVE:MAXVEL?':'10', 'SOUR:WAVE:START?':'1060',
            'SOUR:WAVE:STOP?':'1061', 'SOUR:WAVE:SLEW:FORW?':'1',
            'SOUR:WAVE:SLEW:RET?':'10', 'SOUR:WAVE:DESSCANS?':'1',
            'OUTP:SCAN:START':'OK', 'OUTP:SCAN:STOP':'OK'})

    def query(self, command):
        if command == 'SOUR:WAVE:SLEW:RET 10':
            self.commands.append(command)
            self.replies['SOUR:WAVE:SLEW:RET?'] = self.replies['SOUR:WAVE:MAXVEL?']
            return 'OK'
        return super().query(command)


class ControlsTests(unittest.TestCase):
    def test_motion_completion_brackets_wavelength_and_transition_stays_busy(self):
        device,wire=self.fixture()
        original=wire.query
        count=0
        def completing(command):
            nonlocal count
            if command=='*OPC?':
                count+=1;wire.replies[command]='0' if count==1 else '1'
                if count==2:wire.replies['SENS:WAVE']='1061'
            return original(command)
        wire.query=completing
        moving=device.read_motion()
        self.assertFalse(moving['operation_complete'])
        settled=device.read_motion()
        self.assertTrue(settled['operation_complete'])
        self.assertEqual(settled['wavelength_nm'],1061)
    def fixture(self, head='6722-P'):
        wire=ScanScript(head)
        device=TLB6700(device_key=wire.key, _transport=wire)
        device.connect()
        self.addCleanup(device.close)
        return device,wire

    def test_published_pigtailed_model_automatically_resolves_limits(self):
        device,wire=self.fixture()
        self.assertEqual(device.identity['head_model'],'6722-P')
        self.assertEqual(device.wavelength_range_nm,(1045.,1085.))
        self.assertEqual(device.max_scan_speed_nm_s,10.)

    def test_wavelength_takes_remote_and_tracking_then_returns_panel(self):
        device,wire=self.fixture()
        wire.replies['OUTP:TRAC?']='0'
        device.move_wavelength(1060,confirm=True)
        self.assertEqual(wire.commands[3:],['*OPC?','SYST:MCONT?','OUTP:TRAC?',
            'SYST:MCONT REM','OUTP:TRAC 1','SOUR:WAVE 1060','SYST:MCONT LOC'])

    def test_scan_starts_only_after_verified_settings_without_output_changes(self):
        device,wire=self.fixture()
        device.start_scan(1060,1061,1,confirm=True)
        self.assertEqual([c for c in wire.commands if ' ' in c or c.startswith('OUTP:SCAN')],
            ['SYST:MCONT REM','SOUR:WAVE:START 1060','SOUR:WAVE:STOP 1061',
             'SOUR:WAVE:SLEW:FORW 1','SOUR:WAVE:SLEW:RET 10','SOUR:WAVE:DESSCANS 1',
             'OUTP:SCAN:START','SYST:MCONT LOC'])

    def test_invalid_inputs_unknown_head_and_lower_actual_max_never_write(self):
        device,wire=self.fixture()
        for start,stop,speed in [(1044,1061,1),(1060,1086,1),(1060,1060,1),
            (1060,1061,True),(1060,1061,float('nan')),(1060,1061,11)]:
            with self.assertRaises(InstrumentSafetyError):
                device.start_scan(start,stop,speed,confirm=True)
        wire.replies['SOUR:WAVE:MAXVEL?']='0.5'
        with self.assertRaises(InstrumentSafetyError):device.start_scan(1060,1061,1,confirm=True)
        self.assertFalse(any(' ' in c for c in wire.commands))
        custom,custom_wire=self.fixture('6722-P-EXT')
        with self.assertRaises(InstrumentSafetyError):custom.start_scan(1060,1061,1,confirm=True)
        self.assertEqual(len(custom_wire.commands),3)

    def test_busy_stop_does_not_reset_or_disable_emission(self):
        device,wire=self.fixture()
        wire.replies['*OPC?']='0'
        device.stop_scan(confirm=True)
        self.assertEqual(wire.commands[3:],['SYST:MCONT?','SYST:MCONT REM','OUTP:SCAN:STOP','SYST:MCONT LOC'])

    def test_partial_failure_holds_without_scan_start_cleanup_or_replay(self):
        device,wire=self.fixture()
        wire.replies['SOUR:WAVE:STOP 1061']='VALUE OUT OF RANGE'
        with self.assertRaises(InstrumentProtocolError):device.start_scan(1060,1061,1,confirm=True)
        self.assertEqual(device.state,DriverState.FAULT)
        self.assertNotIn('OUTP:SCAN:START',wire.commands)
        self.assertNotIn('SYST:MCONT LOC',wire.commands)
        self.assertEqual(wire.commands.count('SOUR:WAVE:STOP 1061'),1)

    def test_confirmation_and_output_boolean_checked_before_takeover(self):
        device,wire=self.fixture()
        for fn in (lambda:device.start_scan(1060,1061,1),
                   lambda:device.control_output(1,confirm=True)):
            with self.assertRaises(InstrumentSafetyError):fn()
        self.assertEqual(len(wire.commands),3)

    def test_operator_limits_only_tighten_and_are_applied_to_motion(self):
        wire=ScanScript()
        device=TLB6700(device_key=wire.key,_transport=wire,control_limits={
            'min_nm':1059.,'max_nm':1062.,'max_speed_nm_s':1.})
        device.connect()
        self.addCleanup(device.close)
        with self.assertRaises(InstrumentSafetyError):device.move_wavelength(1063,confirm=True)
        with self.assertRaises(InstrumentSafetyError):device.set_wavelength(1063,confirm=True)
        with self.assertRaises(InstrumentSafetyError):device.start_scan(1060,1061,2,confirm=True)
        device.start_scan(1060,1061,1,confirm=True)
        self.assertIn('SOUR:WAVE:SLEW:RET 1',wire.commands)
        self.assertNotIn('SOUR:WAVE:SLEW:RET 10',wire.commands)

class ReadyModeTests(unittest.TestCase):
    fixture=ControlsTests.fixture
    def test_target_changes_preserve_tracking_off_and_do_not_enable_output(self):
        device,wire=self.fixture();wire.replies['OUTP:TRAC?']='0'
        device.set_target_wavelength(1060.125,confirm=True)
        self.assertIn('SOUR:WAVE 1060.125',wire.commands)
        self.assertNotIn('OUTP:TRAC 1',wire.commands)
        self.assertNotIn('OUTP:STAT 1',wire.commands)
    def test_independent_backward_speed_and_upper_bound(self):
        device,wire=self.fixture()
        with self.assertRaises(InstrumentSafetyError):device.start_scan(1060,1061,1,return_speed_nm_s=11,confirm=True)
        self.assertFalse(any(' ' in c for c in wire.commands))
        device.start_scan(1060,1061,1,return_speed_nm_s=.5,confirm=True)
        self.assertIn('SOUR:WAVE:SLEW:RET 0.5',wire.commands)
    def test_ready_mode_off_can_interrupt_busy_motor(self):
        device,wire=self.fixture();wire.replies['*OPC?']='0'
        device.control_tracking(False,confirm=True)
        self.assertIn('OUTP:TRAC 0',wire.commands)
