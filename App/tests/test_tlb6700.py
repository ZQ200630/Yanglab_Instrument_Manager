"""Real v3 pipeline with a finite, injected Newport transport boundary."""
import dataclasses
import threading
import time
import unittest
import uuid

from App.worker.catalog import load_catalog, CatalogError
from App.worker.controller import DomainController
from App.worker.contracts_v3 import domain_config, RequestV3
from Code.Debugs.test_tlb6700 import Script


def config(number=1, **changes):
    value = dict(domain={'kind':'device', 'id':format(number, '032x')}, config_rev=1,
        driver_kind='laser', model_id='tlb6700', profile_id='newport-usb',
        params={'device_key':'6700 SN1012'}, expected_identity={}, members=[])
    value.update(changes)
    return domain_config(value)


class LaserAppTests(unittest.TestCase):
    def test_following_setting_survives_ready_and_each_target_starts_motion(self):
        controller,cfg=self.controller(),config();controller.configure(cfg)
        self.assertEqual(self.call(controller,cfg,'connect',{}).phase,'completed')
        driver=controller.device(cfg.domain);key='device:'+cfg.domain.id
        self.assertEqual(self.call(controller,cfg,'action',{'name':'control_tracking','args':{'enabled':True,'confirm':True}}).phase,'completed')
        driver._transport.replies['OUTP:TRAC?']='0'
        self.assertEqual(self.call(controller,cfg,'action',{'name':'read_status','args':{}}).phase,'completed')
        status=controller.cached_status()['devices'][key]
        self.assertFalse(status['laser']['tracking']);self.assertTrue(status['target_following_enabled'])
        driver._transport.commands.clear()
        self.assertEqual(self.call(controller,cfg,'action',{'name':'set_target_wavelength','args':{'wavelength_nm':1060.125,'confirm':True}}).phase,'completed')
        self.assertIn('OUTP:TRAC 1',driver._transport.commands)
    def test_target_ack_does_not_wait_for_observation_or_refresh_power_age(self):
        controller,cfg=self.controller(),config()
        controller.configure(cfg)
        self.assertEqual(self.call(controller,cfg,'connect',{}).phase,'completed')
        driver=controller.device(cfg.domain)
        before=controller.cached_status()['devices']['device:'+cfg.domain.id]['laser']
        original=driver._transport.query
        entered,release=threading.Event(),threading.Event()
        def gated(command):
            if command=='SENS:WAVE':
                entered.set()
                if not release.wait(3):raise AssertionError('bounded read gate timed out')
            return original(command)
        driver._transport.query=gated
        try:
            result=self.call(controller,cfg,'action',{'name':'set_target_wavelength','args':{'wavelength_nm':1060,'confirm':True}})
            self.assertEqual(result.phase,'completed',result)
            self.assertTrue(result.result['acknowledged'])
            self.assertTrue(entered.wait(1))
            cached=controller.cached_status()['devices']['device:'+cfg.domain.id]
            self.assertEqual(cached['laser'],before)
            self.assertTrue(cached['motion_pending'])
        finally:release.set()

    def test_fast_motion_updates_do_not_refresh_power_current_or_full_sample_age(self):
        controller,cfg=self.controller(),config()
        controller.configure(cfg)
        self.assertEqual(self.call(controller,cfg,'connect',{}).phase,'completed')
        driver=controller.device(cfg.domain)
        before=controller.cached_status()['devices']['device:'+cfg.domain.id]['laser']
        driver._transport.commands.clear()
        result=self.call(controller,cfg,'action',{'name':'read_motion','args':{}})
        self.assertEqual(result.phase,'completed',result)
        cached=controller.cached_status()['devices']['device:'+cfg.domain.id]
        self.assertEqual(cached['laser'],before)
        self.assertFalse(cached['motion_pending'])
        self.assertEqual(driver._transport.commands,['*OPC?','SENS:WAVE','SOUR:WAVE?','OUTP:TRAC?','*OPC?'])

    def test_laser_cleanup_reports_its_kind_and_retains_failed_attempt(self):
        for close_fails in (False, True):
            with self.subTest(close_fails=close_fails):
                controller, cfg = self.controller(), config()
                controller.configure(cfg)
                self.assertEqual(self.call(controller, cfg, 'connect', {}).phase, 'completed')
                driver = controller.device(cfg.domain)
                before = list(driver._transport.commands)
                driver._transport.close_error = close_fails
                try:
                    result = self.call(controller, cfg, 'disconnect', {})
                    attempt = controller.cached_status()['domain_cleanup_attempts'][-1]
                    self.assertEqual([step['role'] for step in attempt['steps']], ['laser'])
                    self.assertEqual(attempt['steps'][0]['action'], 'close')
                    self.assertEqual(attempt['steps'][0]['ok'], not close_fails)
                    self.assertEqual(bool(attempt['unreleased']), close_fails)
                    self.assertEqual(driver.resources_released, not close_fails)
                    self.assertEqual(driver._transport.commands, before)
                    if close_fails:
                        self.assertEqual(result.phase, 'failed_after_call_started')
                        driver._transport.close_error = False
                        self.assertEqual(self.call(controller, cfg, 'disconnect', {}).phase, 'completed')
                        attempts = controller.cached_status()['domain_cleanup_attempts']
                        self.assertEqual(attempts[-2], attempt)
                        self.assertEqual(attempts[-1]['steps'][0]['role'], 'laser')
                        self.assertEqual(attempts[-1]['unreleased'], [])
                    else:
                        self.assertEqual(result.phase, 'completed')
                finally:
                    driver._transport.close_error = False

    def test_cached_laser_age_advances_while_next_observation_is_blocked(self):
        controller, cfg = self.controller(), config()
        controller.configure(cfg)
        self.assertEqual(self.call(controller, cfg, 'connect', {}).phase, 'completed')
        driver = controller.device(cfg.domain)
        original = driver._transport.query
        entered, release = threading.Event(), threading.Event()
        def gated(command):
            if command == 'SENS:WAVE':
                entered.set()
                if not release.wait(3):
                    raise AssertionError('finite observation gate timed out')
            return original(command)
        driver._transport.query = gated
        pending = controller.submit(RequestV3(uuid.uuid4().hex, 'action',
            {'name':'read_status', 'args':{}}, controller.context(cfg.domain)))
        try:
            self.assertTrue(entered.wait(2))
            key = 'device:' + cfg.domain.id
            before = controller.cached_status()['devices'][key]
            commands = list(driver._transport.commands)
            time.sleep(0.08)
            after = controller.cached_status()['devices'][key]
            self.assertEqual(before['laser']['received_at'], after['laser']['received_at'])
            self.assertGreaterEqual(after['sample_age_s'] - before['sample_age_s'], 0.05)
            self.assertEqual(commands, driver._transport.commands,
                             'Publishing cache age must not acquire hardware')
        finally:
            release.set()
            self.assertEqual(pending.result(5).phase, 'completed')

    def test_catalog_selects_newport_without_visa_or_serial_backend(self):
        model = load_catalog().model('tlb6700')
        self.assertEqual(model.category, 'Laser')
        self.assertEqual(model.profiles['newport-usb'].access, 'newport')
        self.assertNotIn('enable_current', model.operations)
        for params in ({'device_key':'6700 SN1012\n*RST'}, {'device_key':'COM4'},
                       {'device_key':'6700 SN1012', 'dll_path':'user.dll'}):
            with self.assertRaises(CatalogError):
                model.profiles['newport-usb'].validate(params)

    def controller(self):
        from Code.Utils.tlb6700 import TLB6700
        def factory(device_key, **kwargs):
            from Code.Debugs.test_tlb_controls import ScanScript
            wire=ScanScript(head="TLB-6721")
            wire.key=device_key
            wire.replies["*IDN?"]="New_Focus 6700 v2.4 03/19/14 SN"+device_key[7:]
            return TLB6700(device_key=device_key, _transport=wire, **kwargs)
        controller = DomainController(session_id='a'*32, port_enumerator=lambda:(),
            factories={'laser':factory})
        self.addCleanup(controller.close)
        return controller

    def call(self, controller, cfg, method, params):
        return controller.submit(RequestV3(uuid.uuid4().hex, method, params, controller.context(cfg.domain))).result(5)

    def test_connect_observe_typed_action_and_preserving_disconnect(self):
        controller, cfg = self.controller(), config()
        controller.configure(cfg)
        result = self.call(controller, cfg, 'connect', {})
        self.assertEqual(result.phase, 'completed', result)
        driver = controller.device(cfg.domain)
        self.assertFalse(controller.cached_status()['devices']['device:' + cfg.domain.id]['laser']['output_enabled'])
        before = len(driver._transport.commands)
        result = self.call(controller, cfg, 'action', {'name':'read_status', 'args':{}})
        self.assertEqual(result.phase, 'completed', result)
        self.assertEqual(len(driver._transport.commands) - before, 13, 'One refresh must acquire exactly one status sample')
        denied = self.call(controller, cfg, 'action', {'name':'write', 'args':{'command':'*RST'}})
        self.assertEqual(denied.phase, 'rejected_before_call')
        result = self.call(controller, cfg, 'disconnect', {})
        self.assertEqual(result.phase, 'completed', result)
        self.assertTrue(driver.resources_released)
        self.assertTrue(all(' ' not in command for command in driver._transport.commands))

    def test_changed_head_identity_cannot_gain_session_control(self):
        controller = self.controller()
        cfg = config(expected_identity={'model':'TLB-6700','serial':'1012','head_model':'TLB-6721','head_serial':'OTHER'})
        controller.configure(cfg)
        result = self.call(controller, cfg, 'connect', {})
        self.assertNotEqual(result.phase, 'completed')
        self.assertIsNone(controller.device(cfg.domain))

    def test_distinct_lasers_have_independent_domains_and_duplicate_key_is_busy(self):
        controller = self.controller()
        first, second = config(), config(2, params={'device_key':'6700 SN1020'})
        duplicate = config(3)
        for cfg in (first, second, duplicate):
            controller.configure(cfg)
        self.assertEqual(self.call(controller, first, 'connect', {}).phase, 'completed')
        self.assertEqual(self.call(controller, second, 'connect', {}).phase, 'completed')
        self.assertNotEqual(self.call(controller, duplicate, 'connect', {}).phase, 'completed')
        self.assertEqual(self.call(controller, first, 'disconnect', {}).phase, 'completed')
        self.assertEqual(self.call(controller, second, 'action', {'name':'read_status','args':{}}).phase, 'completed')


    def test_typed_scan_and_composite_wavelength_pass_real_pipeline(self):
        controller,cfg=self.controller(),config()
        controller.configure(cfg)
        self.assertEqual(self.call(controller,cfg,'connect',{}).phase,'completed')
        for name,args in [('move_wavelength',{'wavelength_nm':1060,'confirm':True}),
                          ('start_scan',{'start_nm':1060,'stop_nm':1061,'speed_nm_s':1,'confirm':True}),
                          ('stop_scan',{'confirm':True})]:
            result=self.call(controller,cfg,'action',{'name':name,'args':args})
            self.assertEqual(result.phase,'completed',result)
        wire=controller.device(cfg.domain)._transport
        self.assertIn('OUTP:SCAN:START',wire.commands)
        self.assertIn('OUTP:SCAN:STOP',wire.commands)
        self.assertFalse(any(c.startswith('OUTP:STAT ') for c in wire.commands))

    def test_saved_operator_limits_are_enforced_by_driver_pipeline(self):
        controller,cfg=self.controller(),config(params={'device_key':'6700 SN1012',
            'operating_min_nm':1059,'operating_max_nm':1062,'scan_speed_limit_nm_s':0.5})
        controller.configure(cfg)
        self.assertEqual(self.call(controller,cfg,'connect',{}).phase,'completed')
        driver=controller.device(cfg.domain)
        self.assertEqual(driver.operating_range_nm,(1059,1062))
        before=list(driver._transport.commands)
        result=self.call(controller,cfg,'action',{'name':'start_scan','args':{
            'start_nm':1060,'stop_nm':1061,'speed_nm_s':1,'confirm':True}})
        self.assertNotEqual(result.phase,'completed')
        self.assertNotIn('OUTP:SCAN:START',driver._transport.commands[len(before):])

    def test_disconnected_limit_edit_reconfigures_without_rebinding_identity(self):
        controller,cfg=self.controller(),config(expected_identity={'head_model':'TLB-6721'})
        controller.configure(cfg)
        self.assertEqual(self.call(controller,cfg,'connect',{}).phase,'completed')
        self.assertEqual(self.call(controller,cfg,'disconnect',{}).phase,'completed')
        edited=config(config_rev=2,expected_identity=cfg.expected_identity,params={'device_key':'6700 SN1012',
            'operating_min_nm':1059,'operating_max_nm':1062,'scan_speed_limit_nm_s':0.5})
        controller.configure(edited)
        self.assertEqual(self.call(controller,edited,'connect',{}).phase,'completed')
        self.assertEqual(controller.device(edited.domain).operating_max_speed_nm_s,0.5)
        self.assertEqual(self.call(controller,edited,'disconnect',{}).phase,'completed')
        from App.worker.contracts import ProtocolError
        with self.assertRaises(ProtocolError):controller.configure(config(config_rev=3,
            expected_identity=cfg.expected_identity,params=dict(edited.params,device_key='6700 SN1013')))
        with self.assertRaises(ProtocolError):controller.configure(config(config_rev=3,
            expected_identity=cfg.expected_identity,params=dict(edited.params,operating_min_nm=1029)))

if __name__ == '__main__':
    unittest.main()
