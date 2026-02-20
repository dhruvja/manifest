use solana_program::pubkey::Pubkey;
use std::str::FromStr;

const DEFAULT_ER_URL: &str = "https://devnet.magicblock.app";
const DEFAULT_BASE_URL: &str = "https://api.devnet.solana.com";
const DEFAULT_MANIFEST_PROGRAM_ID: &str = "3TN9efyWfeG3s1ZDZdbYtLJwMdWRRtM2xPGsM2T9QrUa";
const DEFAULT_EPHEMERAL_SPL_TOKEN_ID: &str = "SPLxh1LVZzEkX99H6rqYizhytLWPZVV296zyYDPagv2";
const DEFAULT_DELEGATION_PROGRAM_ID: &str = "DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh";
const DEFAULT_PYTH_SOL_USD_FEED: &str = "ENYwebBThHzmzwPLAQvCucUTsjyfBSZdD9ViXksS4jPu";

/// SDK configuration. All fields have sensible devnet defaults.
///
/// # Example
/// ```rust,no_run
/// use manifest_sdk::config::ManifestConfig;
///
/// // Use defaults (devnet)
/// let config = ManifestConfig::default();
///
/// // Custom ER endpoint
/// let config = ManifestConfig::builder()
///     .er_url("https://my-er.example.com")
///     .build();
///
/// // Fully custom
/// let config = ManifestConfig::builder()
///     .base_url("https://api.mainnet-beta.solana.com")
///     .er_url("https://mainnet-er.example.com")
///     .manifest_program_id("MyProgram111111111111111111111111111111111")
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct ManifestConfig {
    /// Base chain RPC URL.
    pub base_url: String,
    /// MagicBlock Ephemeral Rollup RPC URL.
    pub er_url: String,
    /// Manifest DEX program ID.
    pub manifest_program_id: Pubkey,
    /// Ephemeral SPL Token program ID.
    pub ephemeral_spl_token_id: Pubkey,
    /// MagicBlock Delegation program ID.
    pub delegation_program_id: Pubkey,
    /// Default Pyth price feed account.
    pub pyth_feed: Pubkey,
}

impl Default for ManifestConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            er_url: DEFAULT_ER_URL.to_string(),
            manifest_program_id: Pubkey::from_str(DEFAULT_MANIFEST_PROGRAM_ID).unwrap(),
            ephemeral_spl_token_id: Pubkey::from_str(DEFAULT_EPHEMERAL_SPL_TOKEN_ID).unwrap(),
            delegation_program_id: Pubkey::from_str(DEFAULT_DELEGATION_PROGRAM_ID).unwrap(),
            pyth_feed: Pubkey::from_str(DEFAULT_PYTH_SOL_USD_FEED).unwrap(),
        }
    }
}

impl ManifestConfig {
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder::default()
    }
}

/// Builder for [`ManifestConfig`]. Any field left unset uses the devnet default.
#[derive(Debug, Clone, Default)]
pub struct ConfigBuilder {
    base_url: Option<String>,
    er_url: Option<String>,
    manifest_program_id: Option<String>,
    ephemeral_spl_token_id: Option<String>,
    delegation_program_id: Option<String>,
    pyth_feed: Option<String>,
}

impl ConfigBuilder {
    pub fn base_url(mut self, url: &str) -> Self {
        self.base_url = Some(url.to_string());
        self
    }

    pub fn er_url(mut self, url: &str) -> Self {
        self.er_url = Some(url.to_string());
        self
    }

    pub fn manifest_program_id(mut self, id: &str) -> Self {
        self.manifest_program_id = Some(id.to_string());
        self
    }

    pub fn ephemeral_spl_token_id(mut self, id: &str) -> Self {
        self.ephemeral_spl_token_id = Some(id.to_string());
        self
    }

    pub fn delegation_program_id(mut self, id: &str) -> Self {
        self.delegation_program_id = Some(id.to_string());
        self
    }

    pub fn pyth_feed(mut self, feed: &str) -> Self {
        self.pyth_feed = Some(feed.to_string());
        self
    }

    pub fn build(self) -> ManifestConfig {
        let defaults = ManifestConfig::default();
        ManifestConfig {
            base_url: self.base_url.unwrap_or(defaults.base_url),
            er_url: self.er_url.unwrap_or(defaults.er_url),
            manifest_program_id: self
                .manifest_program_id
                .map(|s| Pubkey::from_str(&s).expect("invalid manifest_program_id"))
                .unwrap_or(defaults.manifest_program_id),
            ephemeral_spl_token_id: self
                .ephemeral_spl_token_id
                .map(|s| Pubkey::from_str(&s).expect("invalid ephemeral_spl_token_id"))
                .unwrap_or(defaults.ephemeral_spl_token_id),
            delegation_program_id: self
                .delegation_program_id
                .map(|s| Pubkey::from_str(&s).expect("invalid delegation_program_id"))
                .unwrap_or(defaults.delegation_program_id),
            pyth_feed: self
                .pyth_feed
                .map(|s| Pubkey::from_str(&s).expect("invalid pyth_feed"))
                .unwrap_or(defaults.pyth_feed),
        }
    }
}
