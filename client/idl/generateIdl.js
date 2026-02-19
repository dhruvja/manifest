const bs58 = require('bs58');
const keccak256 = require('keccak256');
const path = require('path');
const fs = require('fs');
const {
  rustbinMatch,
  confirmAutoMessageConsole,
} = require('@metaplex-foundation/rustbin');
const { spawnSync } = require('child_process');

const idlDir = __dirname;
const rootDir = path.join(__dirname, '..', '..', '.crates');

async function main() {
  console.log('Root dir address:', rootDir);
  ['manifest', 'wrapper'].map(async (programName) => {
    const programDir = path.join(
      __dirname,
      '..',
      '..',
      'programs',
      programName,
    );
    const cargoToml = path.join(programDir, 'Cargo.toml');
    console.log('Cargo.Toml address:', cargoToml);

    const rustbinConfig = {
      rootDir,
      binaryName: 'shank',
      binaryCrateName: 'shank-cli',
      libName: 'shank',
      dryRun: false,
      cargoToml,
    };
    // Uses rustbin from https://github.com/metaplex-foundation/rustbin
    const { fullPathToBinary: shankExecutable } = await rustbinMatch(
      rustbinConfig,
      confirmAutoMessageConsole,
    );
    spawnSync(shankExecutable, [
      'idl',
      '--out-dir',
      idlDir,
      '--crate-root',
      programDir,
    ]);
    modifyIdlCore(programName.replace('-', '_'));
  });
}

function genLogDiscriminator(programIdString, accName) {
  return keccak256(
    Buffer.concat([
      Buffer.from(bs58.default.decode(programIdString)),
      Buffer.from('manifest::logs::'),
      Buffer.from(accName),
    ]),
  ).subarray(0, 8);
}

function modifyIdlCore(programName) {
  console.log('Adding arguments to IDL for', programName);
  const generatedIdlPath = path.join(idlDir, `${programName}.json`);
  let idl = JSON.parse(fs.readFileSync(generatedIdlPath, 'utf8'));

  // Shank does not understand the type alias.
  idl = findAndReplaceRecursively(idl, { defined: 'DataIndex' }, 'u32');

  // Since we are not using anchor, we do not have the event macro, and that
  // means we need to manually insert events into idl.
  idl.events = [];

  if (programName == 'manifest') {
    // generateClient does not handle events
    // https://github.com/metaplex-foundation/shank/blob/34d3081208adca8b6b2be2b77db9b1ab4a70f577/shank-idl/src/file.rs#L185
    // so dont remove from accounts
    for (const idlAccount of idl.accounts) {
      if (idlAccount.name.includes('Log')) {
        const event = {
          name: idlAccount.name,
          discriminator: [
            ...genLogDiscriminator(idl.metadata.address, idlAccount.name),
          ],
          fields: [
              ...(idlAccount.type.fields).map((field) => { return { ...field, index: false };}),
          ]
        };
        idl.events.push(event);
      }
    }

    for (const idlAccount of idl.accounts) {
      if (idlAccount.type && idlAccount.type.fields) {
        idlAccount.type.fields = idlAccount.type.fields.map((field) => {
          if (field.type.defined == 'PodBool') {
            field.type = 'bool';
          }
          if (field.type.defined == 'f64') {
            field.type = 'FixedSizeUint8Array';
          }
          return field;
        });
      }
      if (idlAccount.name == 'QuoteAtomsPerBaseAtom') {
        idlAccount.type.fields[0].type = 'u128';
      }
    }

    updateIdlTypes(idl);

    // Update program ID to deployed perps address
    if (!idl.metadata) idl.metadata = {};
    idl.metadata.address = '3TN9efyWfeG3s1ZDZdbYtLJwMdWRRtM2xPGsM2T9QrUa';

    // Remove duplicate instructions (e.g. stale SwapV2 with wrong discriminant).
    // Keep only the last occurrence of each name (the one with correct discriminant).
    {
      const seen = new Set();
      for (let i = idl.instructions.length - 1; i >= 0; i--) {
        const name = idl.instructions[i].name;
        if (seen.has(name)) {
          idl.instructions.splice(i, 1);
        } else {
          seen.add(name);
        }
      }
    }

    // Patch all instruction accounts to match the actual instruction builders.
    // Shank annotations in instruction.rs are stale (still reference spot-market
    // accounts like base_vault/base_mint). The ground truth is the Rust builders
    // in instruction_builders/*.rs.
    const patchInstruction = (name, discriminantValue, accounts) => {
      const existing = idl.instructions.find((i) => i.name === name);
      if (existing) {
        existing.discriminant = { type: 'u8', value: discriminantValue };
        existing.accounts = accounts;
      } else {
        idl.instructions.push({ name, discriminant: { type: 'u8', value: discriminantValue }, accounts, args: [] });
      }
    };

    // 0 - CreateMarket: perps market PDA from (base_mint_index, quote_mint)
    patchInstruction('CreateMarket', 0, [
      { name: 'payer', isMut: true, isSigner: true, docs: ['Market creator / payer'] },
      { name: 'market', isMut: true, isSigner: false, docs: ['Market PDA, seeds=[b"market", &[base_mint_index], quote_mint]'] },
      { name: 'systemProgram', isMut: false, isSigner: false, docs: ['System program'] },
      { name: 'quoteMint', isMut: false, isSigner: false, docs: ['Quote mint (e.g. USDC)'] },
      { name: 'quoteVault', isMut: true, isSigner: false, docs: ['Quote vault ATA owned by market PDA'] },
      { name: 'tokenProgram', isMut: false, isSigner: false, docs: ['SPL Token program'] },
      { name: 'tokenProgram22', isMut: false, isSigner: false, docs: ['SPL Token-2022 program'] },
      { name: 'associatedTokenProgram', isMut: false, isSigner: false, docs: ['Associated Token Account program'] },
      { name: 'ephemeralVaultAta', isMut: true, isSigner: false, docs: ['Ephemeral SPL token vault ATA'] },
      { name: 'ephemeralSplToken', isMut: false, isSigner: false, docs: ['Ephemeral SPL Token program (MagicBlock)'] },
    ]);

    // 1 - ClaimSeat: matches builders
    patchInstruction('ClaimSeat', 1, [
      { name: 'payer', isMut: true, isSigner: true, docs: ['Payer'] },
      { name: 'market', isMut: true, isSigner: false, docs: ['Account holding all market state'] },
      { name: 'systemProgram', isMut: false, isSigner: false, docs: ['System program'] },
    ]);

    // 2 - Deposit: payer is readonly in builder
    patchInstruction('Deposit', 2, [
      { name: 'payer', isMut: false, isSigner: true, docs: ['Payer'] },
      { name: 'market', isMut: true, isSigner: false, docs: ['Account holding all market state'] },
      { name: 'traderToken', isMut: true, isSigner: false, docs: ['Trader token account'] },
      { name: 'vault', isMut: true, isSigner: false, docs: ['Vault PDA, seeds are [b\'vault\', market, mint]'] },
      { name: 'tokenProgram', isMut: false, isSigner: false, docs: ['Token program'] },
      { name: 'mint', isMut: false, isSigner: false, docs: ['Quote mint'] },
    ]);

    // 3 - Withdraw: payer is readonly in builder
    patchInstruction('Withdraw', 3, [
      { name: 'payer', isMut: false, isSigner: true, docs: ['Payer'] },
      { name: 'market', isMut: true, isSigner: false, docs: ['Account holding all market state'] },
      { name: 'traderToken', isMut: true, isSigner: false, docs: ['Trader token account'] },
      { name: 'vault', isMut: true, isSigner: false, docs: ['Vault PDA, seeds are [b\'vault\', market, mint]'] },
      { name: 'tokenProgram', isMut: false, isSigner: false, docs: ['Token program'] },
      { name: 'mint', isMut: false, isSigner: false, docs: ['Quote mint'] },
    ]);

    // 4 - Swap: perps only uses quote side (base is virtual)
    patchInstruction('Swap', 4, [
      { name: 'payer', isMut: false, isSigner: true, docs: ['Payer'] },
      { name: 'market', isMut: true, isSigner: false, docs: ['Account holding all market state'] },
      { name: 'systemProgram', isMut: false, isSigner: false, docs: ['System program'] },
      { name: 'traderQuote', isMut: true, isSigner: false, docs: ['Trader quote token account'] },
      { name: 'quoteVault', isMut: true, isSigner: false, docs: ['Quote vault PDA'] },
      { name: 'tokenProgramQuote', isMut: false, isSigner: false, docs: ['Token program for quote'] },
    ]);

    // 5 - Expand: matches builders
    patchInstruction('Expand', 5, [
      { name: 'payer', isMut: true, isSigner: true, docs: ['Payer'] },
      { name: 'market', isMut: true, isSigner: false, docs: ['Account holding all market state'] },
      { name: 'systemProgram', isMut: false, isSigner: false, docs: ['System program'] },
    ]);

    // 6 - BatchUpdate: matches builders (global accounts are optional, included dynamically)
    patchInstruction('BatchUpdate', 6, [
      { name: 'payer', isMut: true, isSigner: true, docs: ['Payer'] },
      { name: 'market', isMut: true, isSigner: false, docs: ['Account holding all market state'] },
      { name: 'systemProgram', isMut: false, isSigner: false, docs: ['System program'] },
    ]);

    // 7 - GlobalCreate: matches builders
    patchInstruction('GlobalCreate', 7, [
      { name: 'payer', isMut: true, isSigner: true, docs: ['Payer'] },
      { name: 'global', isMut: true, isSigner: false, docs: ['Global account'] },
      { name: 'systemProgram', isMut: false, isSigner: false, docs: ['System program'] },
      { name: 'mint', isMut: false, isSigner: false, docs: ['Mint for this global account'] },
      { name: 'globalVault', isMut: true, isSigner: false, docs: ['Global vault'] },
      { name: 'tokenProgram', isMut: false, isSigner: false, docs: ['Token program'] },
    ]);

    // 8 - GlobalAddTrader: matches builders
    patchInstruction('GlobalAddTrader', 8, [
      { name: 'payer', isMut: true, isSigner: true, docs: ['Payer'] },
      { name: 'global', isMut: true, isSigner: false, docs: ['Global account'] },
      { name: 'systemProgram', isMut: false, isSigner: false, docs: ['System program'] },
    ]);

    // 9 - GlobalDeposit: payer is readonly in builder
    patchInstruction('GlobalDeposit', 9, [
      { name: 'payer', isMut: false, isSigner: true, docs: ['Payer'] },
      { name: 'global', isMut: true, isSigner: false, docs: ['Global account'] },
      { name: 'mint', isMut: false, isSigner: false, docs: ['Mint for this global account'] },
      { name: 'globalVault', isMut: true, isSigner: false, docs: ['Global vault'] },
      { name: 'traderToken', isMut: true, isSigner: false, docs: ['Trader token account'] },
      { name: 'tokenProgram', isMut: false, isSigner: false, docs: ['Token program'] },
    ]);

    // 10 - GlobalWithdraw: payer is readonly in builder
    patchInstruction('GlobalWithdraw', 10, [
      { name: 'payer', isMut: false, isSigner: true, docs: ['Payer'] },
      { name: 'global', isMut: true, isSigner: false, docs: ['Global account'] },
      { name: 'mint', isMut: false, isSigner: false, docs: ['Mint for this global account'] },
      { name: 'globalVault', isMut: true, isSigner: false, docs: ['Global vault'] },
      { name: 'traderToken', isMut: true, isSigner: false, docs: ['Trader token account'] },
      { name: 'tokenProgram', isMut: false, isSigner: false, docs: ['Token program'] },
    ]);

    // 11 - GlobalEvict: trader_token and evictee_token are writable in builder
    patchInstruction('GlobalEvict', 11, [
      { name: 'payer', isMut: true, isSigner: true, docs: ['Payer'] },
      { name: 'global', isMut: true, isSigner: false, docs: ['Global account'] },
      { name: 'mint', isMut: false, isSigner: false, docs: ['Mint for this global account'] },
      { name: 'globalVault', isMut: true, isSigner: false, docs: ['Global vault'] },
      { name: 'traderToken', isMut: true, isSigner: false, docs: ['Trader token account'] },
      { name: 'evicteeToken', isMut: true, isSigner: false, docs: ['Evictee token account'] },
      { name: 'tokenProgram', isMut: false, isSigner: false, docs: ['Token program'] },
    ]);

    // 12 - GlobalClean: matches builders
    patchInstruction('GlobalClean', 12, [
      { name: 'payer', isMut: true, isSigner: true, docs: ['Payer for this tx, receiver of rent deposit'] },
      { name: 'market', isMut: true, isSigner: false, docs: ['Account holding all market state'] },
      { name: 'systemProgram', isMut: false, isSigner: false, docs: ['System program'] },
      { name: 'global', isMut: true, isSigner: false, docs: ['Global account'] },
    ]);

    // 13 - SwapV2: perps only uses quote side, separates payer and owner
    patchInstruction('SwapV2', 13, [
      { name: 'payer', isMut: false, isSigner: true, docs: ['Payer (gas)'] },
      { name: 'owner', isMut: false, isSigner: true, docs: ['Owner of the token accounts'] },
      { name: 'market', isMut: true, isSigner: false, docs: ['Account holding all market state'] },
      { name: 'systemProgram', isMut: false, isSigner: false, docs: ['System program'] },
      { name: 'traderQuote', isMut: true, isSigner: false, docs: ['Trader quote token account'] },
      { name: 'quoteVault', isMut: true, isSigner: false, docs: ['Quote vault PDA'] },
      { name: 'tokenProgramQuote', isMut: false, isSigner: false, docs: ['Token program for quote'] },
    ]);

    // 14 - DelegateMarket
    patchInstruction('DelegateMarket', 14, [
      { name: 'payer', isMut: true, isSigner: true, docs: ['Payer and market creator'] },
      { name: 'market', isMut: true, isSigner: false, docs: ['Market PDA to delegate'] },
      { name: 'ownerProgram', isMut: false, isSigner: false, docs: ['Manifest program (owner of the PDA)'] },
      { name: 'delegationProgram', isMut: false, isSigner: false, docs: ['MagicBlock delegation program'] },
      { name: 'delegationRecord', isMut: true, isSigner: false, docs: ['Delegation record PDA'] },
      { name: 'delegationMetadata', isMut: true, isSigner: false, docs: ['Delegation metadata PDA'] },
      { name: 'systemProgram', isMut: false, isSigner: false, docs: ['System program'] },
      { name: 'buffer', isMut: true, isSigner: false, docs: ['Buffer account for delegation'] },
    ]);

    // 15 - CommitMarket
    patchInstruction('CommitMarket', 15, [
      { name: 'payer', isMut: true, isSigner: true, docs: ['Payer'] },
      { name: 'market', isMut: true, isSigner: false, docs: ['Delegated market account'] },
      { name: 'magicProgram', isMut: false, isSigner: false, docs: ['MagicBlock magic program'] },
      { name: 'magicContext', isMut: false, isSigner: false, docs: ['MagicBlock magic context'] },
    ]);

    // 16 - Liquidate
    patchInstruction('Liquidate', 16, [
      { name: 'liquidator', isMut: true, isSigner: true, docs: ['Liquidator'] },
      { name: 'market', isMut: true, isSigner: false, docs: ['Perps market account'] },
      { name: 'systemProgram', isMut: false, isSigner: false, docs: ['System program'] },
    ]);

    // 17 - CrankFunding
    patchInstruction('CrankFunding', 17, [
      { name: 'payer', isMut: true, isSigner: true, docs: ['Payer / cranker'] },
      { name: 'market', isMut: true, isSigner: false, docs: ['Perps market account'] },
      { name: 'pythPriceFeed', isMut: false, isSigner: false, docs: ['Pyth price feed account'] },
    ]);

    // 18 - ReleaseSeat
    patchInstruction('ReleaseSeat', 18, [
      { name: 'payer', isMut: true, isSigner: true, docs: ['Payer / trader releasing seat'] },
      { name: 'market', isMut: true, isSigner: false, docs: ['Account holding all market state'] },
      { name: 'systemProgram', isMut: false, isSigner: false, docs: ['System program'] },
    ]);

    for (const instruction of idl.instructions) {
      // Reset args so the script is idempotent across multiple runs
      instruction.args = [];
      switch (instruction.name) {
        case 'CreateMarket': {
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'CreateMarketParams',
            },
          });
          break;
        }
        case 'ClaimSeat': {
          // Claim seat does not have params
          break;
        }
        case 'Deposit': {
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'DepositParams',
            },
          });
          instruction.args.push({
            "name": "traderIndexHint",
            "type": {
              "option": "u32"
            }
          });
          break;
        }
        case 'Withdraw': {
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'WithdrawParams',
            },
          });
          instruction.args.push({
            "name": "traderIndexHint",
            "type": {
              "option": "u32"
            }
          });
          break;
        }
        case 'Swap': {
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'SwapParams',
            },
          });
          break;
        }
        case 'SwapV2': {
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'SwapParams',
            },
          });
          break;
        }
        case 'BatchUpdate': {
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'BatchUpdateParams',
            },
          });
          break;
        }
        case 'Expand': {
          break;
        }
        case 'GlobalCreate': {
          break;
        }
        case 'GlobalAddTrader': {
          break;
        }
        case 'ReleaseSeat': {
          // Release seat does not have params
          break;
        }
        case 'GlobalDeposit': {
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'GlobalDepositParams',
            },
          });
          break;
        }
        case 'GlobalWithdraw': {
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'GlobalWithdrawParams',
            },
          });
          break;
        }
        case 'GlobalEvict': {
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'GlobalEvictParams',
            },
          });
          break;
        }
        case 'GlobalClean':
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'GlobalCleanParams',
            },
          });
          break;
        case 'DelegateMarket': {
          break;
        }
        case 'CommitMarket': {
          break;
        }
        case 'CrankFunding': {
          break;
        }
        case 'Liquidate': {
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'LiquidateParams',
            },
          });
          break;
        }
        default: {
          console.log(instruction);
          throw new Error('Unexpected instruction');
        }
      }
    }

    // Return type has a tuple which anchor does not support
    idl.types = idl.types.filter((idlType) => idlType.name != "BatchUpdateReturn");

    // LiquidateParams contains a Pubkey field which shank cannot handle automatically
    if (!idl.types.find((t) => t.name === 'LiquidateParams')) {
      idl.types.push({
        name: 'LiquidateParams',
        type: {
          kind: 'struct',
          fields: [
            {
              name: 'traderToLiquidate',
              type: 'publicKey',
            },
          ],
        },
      });
    }

    // CreateMarketParams — perps market creation params (shank cannot extract due to Pubkey field)
    if (!idl.types.find((t) => t.name === 'CreateMarketParams')) {
      idl.types.push({
        name: 'CreateMarketParams',
        type: {
          kind: 'struct',
          fields: [
            { name: 'baseMintIndex', type: 'u8' },
            { name: 'baseMintDecimals', type: 'u8' },
            { name: 'initialMarginBps', type: 'u64' },
            { name: 'maintenanceMarginBps', type: 'u64' },
            { name: 'pythFeedAccount', type: 'publicKey' },
            { name: 'takerFeeBps', type: 'u64' },
            { name: 'liquidationBufferBps', type: 'u64' },
            { name: 'numBlocks', type: 'u32' },
          ],
        },
      });
    }

  } else if (programName == 'wrapper') {
    idl.types.push({
      name: 'WrapperDepositParams',
      type: {
        kind: 'struct',
        fields: [
          {
            name: 'amountAtoms',
            type: 'u64',
          },
        ],
      },
    });
    idl.types.push({
      name: 'WrapperWithdrawParams',
      type: {
        kind: 'struct',
        fields: [
          {
            name: 'amountAtoms',
            type: 'u64',
          },
        ],
      },
    });
    idl.types.push({
      name: 'OrderType',
      type: {
        kind: 'enum',
        variants: [
          { name: 'Limit' },
          { name: 'ImmediateOrCancel' },
          { name: 'PostOnly' },
          { name: 'Global' },
          { name: 'Reverse' },
          { name: 'ReverseTight' },
        ],
      },
    });

    updateIdlTypes(idl);

    for (const instruction of idl.instructions) {
      switch (instruction.name) {
        case 'CreateWrapper': {
          break;
        }
        case 'ClaimSeat': {
          // Claim seat does not have params
          break;
        }
        case 'Deposit': {
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'WrapperDepositParams',
            },
          });
          break;
        }
        case 'Withdraw': {
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'WrapperWithdrawParams',
            },
          });
          break;
        }
        case 'BatchUpdate': {
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'WrapperBatchUpdateParams',
            },
          });
          break;
        }
        case 'BatchUpdateBaseGlobal': {
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'WrapperBatchUpdateParams',
            },
          });
          break;
        }
        case 'BatchUpdateQuoteGlobal': {
          instruction.args.push({
            name: 'params',
            type: {
              defined: 'WrapperBatchUpdateParams',
            },
          });
          break;
        }
        case 'Expand': {
          break;
        }
        case 'Collect': {
          break;
        }
        default: {
          console.log(instruction);
          throw new Error('Unexpected instruction');
        }
      }
    }
  } else {
    throw new Error('Unexpected program name');
  }
  fs.writeFileSync(generatedIdlPath, JSON.stringify(idl, null, 2));
}

function isObject(x) {
  return x instanceof Object;
}

function isArray(x) {
  return x instanceof Array;
}

/**
 * @param {*} target Target can be anything
 * @param {*} find val to find
 * @param {*} replaceWith val to replace
 * @returns the target with replaced values
 */
function findAndReplaceRecursively(target, find, replaceWith) {
  if (!isObject(target)) {
    if (target === find) {
      return replaceWith;
    }
    return target;
  } else if (
    isObject(find) &&
    JSON.stringify(target) === JSON.stringify(find)
  ) {
    return replaceWith;
  }
  if (isArray(target)) {
    return target.map((child) => {
      return findAndReplaceRecursively(child, find, replaceWith);
    });
  }
  return Object.keys(target).reduce((carry, key) => {
    const val = target[key];
    carry[key] = findAndReplaceRecursively(val, find, replaceWith);
    return carry;
  }, {});
}

function updateIdlTypes(idl) {
  for (const idlType of idl.types) {
    if (idlType.type && idlType.type.fields) {
      idlType.type.fields = idlType.type.fields.map((field) => {
        if (field.type.defined == 'PodBool') {
          field.type = 'bool';
        }
        if (field.type.defined == 'BaseAtoms') {
          field.type = 'u64';
        }
        if (field.type.defined == 'QuoteAtoms') {
          field.type = 'u64';
        }
        if (field.type.defined == 'GlobalAtoms') {
          field.type = 'u64';
        }
        if (field.type.defined == 'QuoteAtomsPerBaseAtom') {
          field.type = 'u128';
        }
        return field;
      });
    }
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
