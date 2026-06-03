import os
import logging
from dataclasses import dataclass
from pathlib import Path


@dataclass
class Config:
    """Application configuration loaded from environment variables."""

    # API
    BASE_URL: str = "https://history.deribit.com/api/v2/public"
    CURRENCY: str = "BTC"
    # Asset vs query-currency split for USDC-linear alts (e.g. SOL/XRP). CURRENCY is the
    # ASSET label used for paths/DB. QUERY_CURRENCY is the Deribit /get_instruments
    # `currency` arg; BASE_CURRENCY is the base_currency to keep. Both default to CURRENCY,
    # a no-op for coin-settled BTC/ETH.
    QUERY_CURRENCY: str = ""
    BASE_CURRENCY: str = ""
    # Optional instrument-lifetime window (epoch ms): keep only contracts whose life
    # overlaps [WINDOW_START_MS, WINDOW_END_MS). None = no filter (original full-history).
    WINDOW_START_MS: int | None = None
    WINDOW_END_MS: int | None = None
    CHUNK_SIZE: int = 10000

    # Paths
    BASE_DIR: Path = Path("./data") / CURRENCY
    DATA_FUTURE_DIR: Path = BASE_DIR / "future"
    DATA_OPTION_DIR: Path = BASE_DIR / "option"
    FUTURE_DB_PATH: Path = BASE_DIR / "future.db"
    OPTION_DB_PATH: Path = BASE_DIR / "option.db"

    # Concurrency & Limits
    MAX_RPS: int = 20
    MAX_WORKERS: int = 40

    # Network
    PROXY: str | None = None

    def __post_init__(self):
        # Override CURRENCY from environment if set
        env_currency = os.environ.get("CURRENCY")
        if env_currency:
            self.CURRENCY = env_currency.strip()

        # Resolve query/base currency (default to the asset CURRENCY = no-op for BTC/ETH).
        # For USDC-linear alts: set CURRENCY=SOL QUERY_CURRENCY=USDC BASE_CURRENCY=SOL.
        self.QUERY_CURRENCY = (os.environ.get("QUERY_CURRENCY") or self.CURRENCY).strip()
        self.BASE_CURRENCY = (os.environ.get("BASE_CURRENCY") or self.CURRENCY).strip()

        ws = os.environ.get("WINDOW_START_MS")
        we = os.environ.get("WINDOW_END_MS")
        self.WINDOW_START_MS = int(ws) if ws else None
        self.WINDOW_END_MS = int(we) if we else None

        # Recompute paths based on actual CURRENCY value
        # (BASE_DIR and derivatives are computed at class definition time,
        #  so they must be recalculated here to reflect env var overrides)
        # DATA_ROOT lets a window-bounded run write to a fresh root so its
        # per-instrument .done markers never collide with a prior full-history
        # run in ./data. Defaults to ./data (original behavior).
        data_root = os.environ.get("DATA_ROOT")
        self.BASE_DIR = (Path(data_root) if data_root else Path("./data")) / self.CURRENCY
        self.DATA_FUTURE_DIR = self.BASE_DIR / "future"
        self.DATA_OPTION_DIR = self.BASE_DIR / "option"
        self.FUTURE_DB_PATH = self.BASE_DIR / "future.db"
        self.OPTION_DB_PATH = self.BASE_DIR / "option.db"

        # Resolve proxy from environment (try uppercase and lowercase variants)
        self.PROXY = (
            os.environ.get("HTTP_PROXY")
            or os.environ.get("HTTPS_PROXY")
            or os.environ.get("http_proxy")
            or os.environ.get("https_proxy")
        )
        if self.PROXY:
            self.PROXY = self.PROXY.strip()


logger = logging.getLogger("Deribit Fetcher")

settings = Config()
