import { BN, NearAccount, captureError, toYocto, tGas, DEFAULT_FUNCTION_CALL_GAS, Gas, NEAR } from 'near-workspaces-ava';
import { workspace, initStaking, initTestToken, STORAGE_PER_BYTE, workspaceWithoutInit } from './utils';
import { voteApprove } from './proposals.ava';
import { DEADLINE, BOND, proposeBounty, voteOnBounty, claimBounty, doneBounty } from './bounties.ava'
import * as fs from 'fs';

const DAO_WASM_BYTES: Uint8Array = fs.readFileSync('../res/sputnikdao2.wasm');

workspace.test('Upgrade self', async (test, { root, dao }) => {
    const result = await root
        .createTransaction(dao)
        .functionCall(
            'store_blob',
            DAO_WASM_BYTES,
            {
                attachedDeposit: toYocto('200'),
                gas: tGas(300),
            })
        .signAndSend();
    const hash = result.parseResult<String>()
    const proposalId = await root.call(
        dao,
        'add_proposal',
        {
            proposal:
            {
                description: 'test',
                kind: { "UpgradeSelf": { hash: hash } }
            }
        },
        {
            attachedDeposit: toYocto('1'),
        }
    );


    const id: number = await dao.view('get_last_proposal_id');
    test.is(id, 1);

    await root.call(
        dao,
        'act_proposal',
        {
            id: proposalId,
            action: 'VoteApprove',
        },
        {
            gas: tGas(300), // attempt to subtract with overflow if not enough gas, maybe add some checks?
        }
    );

    test.is(await dao.view('version'), "2.0.0");

    const beforeBlobRemove = new BN(await dao.view('get_available_amount'));
    await root.call(
        dao,
        'remove_blob',
        {
            hash: hash,
        }
    );
    test.assert(
        new BN(await dao.view('get_available_amount')).gt(beforeBlobRemove)
    )
});


workspaceWithoutInit.test('Upgrade self negative', async (test, { root, dao }) => {
    const config = { name: 'sputnik', purpose: 'testing', metadata: '' };

    // NOT INITIALIZED
    let err = await captureError(async () =>
        root
            .createTransaction(dao)
            .functionCall(
                'store_blob',
                DAO_WASM_BYTES,
                {
                    attachedDeposit: toYocto('200'),
                    gas: tGas(300),
                })
            .signAndSend()
    );
    test.regex(err, /ERR_CONTRACT_IS_NOT_INITIALIZED/);

    // Initializing contract
    await root.call(
        dao,
        'new',
        { config, policy: [root.accountId] },
    );

    // not enough deposit
    err = await captureError(async () =>
        root
            .createTransaction(dao)
            .functionCall(
                'store_blob',
                DAO_WASM_BYTES,
                {
                    attachedDeposit: toYocto('1'),
                    gas: tGas(300),
                })
            .signAndSend()
    );
    test.regex(err, /ERR_NOT_ENOUGH_DEPOSIT/);

    await root
        .createTransaction(dao)
        .functionCall(
            'store_blob',
            DAO_WASM_BYTES,
            {
                attachedDeposit: toYocto('5'),
                gas: tGas(300),
            })
        .signAndSend();

    // Already exists
    err = await captureError(async () =>
        root
            .createTransaction(dao)
            .functionCall(
                'store_blob',
                DAO_WASM_BYTES,
                {
                    attachedDeposit: toYocto('200'),
                    gas: tGas(300),
                })
            .signAndSend()
    );
    test.regex(err, /ERR_ALREADY_EXISTS/);

});

workspace.test('Remove blob', async (test, { root, dao, alice }) => {
    const result = await root
        .createTransaction(dao)
        .functionCall(
            'store_blob',
            DAO_WASM_BYTES,
            {
                attachedDeposit: toYocto('200'),
                gas: tGas(300),
            })
        .signAndSend();

    const hash = result.parseResult<String>()
    
    // fails if hash is wrong
    let err = await captureError(async () =>
        root.call(
            dao,
            'remove_blob',
            {
                hash: "HLBiX51txizmQzZJMrHMCq4u7iEEqNbaJppZ84yW7628", // some_random hash
            }
        )
    );
    test.regex(err, /ERR_NO_BLOB/);

    // Can only be called by the original storer
    err = await captureError(async () =>
        alice.call(
            dao,
            'remove_blob',
            {
                hash: hash,
            }
        )
    );
    test.regex(err, /ERR_INVALID_CALLER/);

    // blob is removed with payback
    const rootAmountBeforeRemove = (await root.balance()).total
    await root.call(
        dao,
        'remove_blob',
        {
            hash: hash,
        }
    );
    const rootAmountAfterRemove = (await root.balance()).total
    test.false(await dao.view('has_blob', { hash: hash }));
    test.assert(rootAmountAfterRemove.gt(rootAmountBeforeRemove));
});

workspace.test('Callback for BountyDone', async (test, { alice, root, dao }) => {
    //During the callback the number bounty_claims_count should decrease
    const proposalId = await proposeBounty(alice, dao);
    await voteOnBounty(root, dao, proposalId);
    await claimBounty(alice, dao, proposalId);
    await doneBounty(alice, alice, dao, proposalId);
    //Before the bounty is done there is 1 claim
    test.is(await dao.view('get_bounty_number_of_claims', {id: 0}), 1);
    const balanceBefore: NEAR = (await alice.balance()).total;
    //During the callback this number is decreased
    await voteOnBounty(root, dao, proposalId + 1);
    const balanceAfter: NEAR = (await alice.balance()).total;
    test.is(await dao.view('get_bounty_number_of_claims', {id: 0}), 0);
    test.assert(balanceBefore.lt(balanceAfter));
});

workspace.test('Callback transfer', async (test, { alice, root, dao }) => {
    const user1 = await root.createAccount('user1');
    // Fail transfer by transfering to non-existent accountId
    let transferId: number = await user1.call(
        dao,
        'add_proposal', {
        proposal: {
            description: 'give me tokens',
            kind: {
                Transfer: {
                    token_id: "",
                    receiver_id: "broken_id",
                    amount: toYocto('1'),
                }
            }
        },
    }, { attachedDeposit: toYocto('1') });
    let user1Balance = (await user1.balance()).total
    await voteApprove(root, dao, transferId);
    let { status } = await dao.view('get_proposal', { id: transferId });
    test.is(status, 'Failed');
    test.assert((await user1.balance()).total.eq(user1Balance)); // no bond returns on fail

    // now we transfer to real accountId
    transferId = await user1.call(
        dao,
        'add_proposal', {
        proposal: {
            description: 'give me tokens',
            kind: {
                Transfer: {
                    token_id: "",
                    receiver_id: alice.accountId, // valid id this time
                    amount: toYocto('1'),
                }
            }
        },
    }, { attachedDeposit: toYocto('1') });
    user1Balance = (await user1.balance()).total
    await voteApprove(root, dao, transferId);
    ({ status } = await dao.view('get_proposal', { id: transferId }));
    test.is(status, 'Approved');
    test.assert((await user1.balance()).total.gt(user1Balance)); // returns bond
});

workspace.test('Callback function call', async (test, { alice, root, dao }) => {
    const testToken = await initTestToken(root);
    let transferId: number = await root.call(
        dao,
        'add_proposal', {
        proposal: {
            description: 'give me tokens',
            kind: {
                FunctionCall: {
                    receiver_id: testToken.accountId,
                    actions: [{ method_name: 'fail', args: Buffer.from('bad args').toString('base64'), deposit: toYocto('1'), gas: tGas(10) }],
                }
            }
        },
    }, { attachedDeposit: toYocto('1') });
    await root.call(dao, 'act_proposal',
        {
            id: transferId,
            action: 'VoteApprove'
        },
        {
            gas: tGas(200),
        });
    let { status } = await dao.view('get_proposal', { id: transferId });
    test.is(status, 'Failed');

    transferId = await root.call(
        dao,
        'add_proposal', {
        proposal: {
            description: 'give me tokens',
            kind: {
                FunctionCall: {
                    receiver_id: testToken.accountId,
                    actions: [
                        { method_name: 'mint', args: Buffer.from('{"account_id": "' + alice.accountId + '", "amount": "10"}').toString('base64'), deposit: '0', gas: tGas(10) },
                        { method_name: 'burn', args: Buffer.from('{"account_id": "' + alice.accountId + '", "amount": "10"}').toString('base64'), deposit: '0', gas: tGas(10) }],
                }
            }
        },
    }, { attachedDeposit: toYocto('1') });
    await root.call(dao, 'act_proposal',
        {
            id: transferId,
            action: 'VoteApprove'
        },
        {
            gas: tGas(200),
        });
    ({ status } = await dao.view('get_proposal', { id: transferId }));
    test.is(status, 'Approved');
});