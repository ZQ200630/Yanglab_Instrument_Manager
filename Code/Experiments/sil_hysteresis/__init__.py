"""SIL hysteresis experiment configuration, orchestration, and CLI."""

__all__ = ["DriverFactories", "cli_main"]


def __getattr__(name):
    """Load CLI exports lazily so ``python -m ...run`` remains warning-free."""

    if name in __all__:
        from .run import DriverFactories, main

        return {"DriverFactories": DriverFactories, "cli_main": main}[name]
    raise AttributeError(name)
