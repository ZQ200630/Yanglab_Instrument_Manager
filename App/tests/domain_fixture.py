"""Configuration-only unit fixture; no factories or instrument I/O."""
from App.worker.contracts_v3 import domain_config

def config(number=1,kind="gain",revision=1):
    model,profile,params = {
        "gain":("gain","cp210x-serial",{"port":f"COM{number}"}),
        "voltage":("voltage","ch340-serial",{"port":f"COM{number}"}),
        "osa":("aq6370","gpib-visa",{"resource":f"GPIB0::{number%30}::INSTR"}),
        "mdt":("mdt693b","serial",{"port":f"COM{number}"}),
    }[kind]
    return domain_config(dict(domain={"kind":"device","id":f"{number:032x}"},config_rev=revision,
        driver_kind=kind,model_id=model,profile_id=profile,params=params,expected_identity={"model":model},members=[]))
