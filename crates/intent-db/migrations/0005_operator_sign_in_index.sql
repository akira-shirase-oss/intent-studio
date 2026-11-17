-- N2 / D-052: "has this address signed in successfully in the last 30 days?" is asked on every
-- sign-in that reaches verification.
CREATE INDEX operator_audit_sign_ins ON operator_audit (address, at) WHERE action = 'operator.sign_in';
